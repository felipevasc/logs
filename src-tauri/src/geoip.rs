//! Offline IP → country, ASN and hosting lookup. Nothing is sent anywhere:
//! the bases are files the user imports (or downloads once, on request) into
//! the application folder. Supported formats:
//! - iptoasn.com `ip2asn-combined.tsv` / `ip2asn-v4.tsv` / `ip2asn-v6.tsv` (optionally gzip);
//! - DB-IP Lite CSV, country (`start,end,CC`) and ASN (`start,end,ASN,organization`).
//!
//! Without a base, no event receives geographic fields and analyses are unchanged.
use parking_lot::RwLock;
use serde::Serialize;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const DOWNLOAD_URL: &str = "https://iptoasn.com/data/ip2asn-combined.tsv.gz";
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Default)]
struct Info {
    asn: u32,
    country: [u8; 2],
    org: u32,
}

#[derive(Default)]
struct Layer {
    v4: Vec<(u32, u32, Info)>,
    v6: Vec<(u128, u128, Info)>,
}

impl Layer {
    fn sort(&mut self) {
        self.v4.sort_unstable_by_key(|r| r.0);
        self.v6.sort_unstable_by_key(|r| r.0);
    }
    fn find(&self, ip: IpAddr) -> Option<Info> {
        fn search<T: Ord + Copy>(ranges: &[(T, T, Info)], ip: T) -> Option<Info> {
            let index = ranges.partition_point(|r| r.0 <= ip);
            let (start, end, info) = *ranges.get(index.checked_sub(1)?)?;
            (start <= ip && ip <= end).then_some(info)
        }
        match ip {
            IpAddr::V4(v4) => search(&self.v4, u32::from(v4)),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => search(&self.v4, u32::from(v4)),
                None => search(&self.v6, u128::from(v6)),
            },
        }
    }
}

pub struct Database {
    layers: Vec<Layer>,
    orgs: Vec<String>,
    signature: String,
    pub files: Vec<FileInfo>,
}

#[derive(Clone, Serialize)]
pub struct FileInfo {
    pub name: String,
    pub bytes: u64,
    pub ranges: usize,
    pub modified_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Location {
    pub country: Option<String>,
    pub asn: Option<u32>,
    pub org: Option<String>,
    pub hosting: bool,
}

#[derive(Default)]
struct Builder {
    orgs: Vec<String>,
    index: std::collections::HashMap<String, u32>,
}
impl Builder {
    fn org(&mut self, name: &str) -> u32 {
        let name = name.trim();
        if name.is_empty() {
            return 0;
        }
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        if self.orgs.is_empty() {
            self.orgs.push(String::new());
        }
        let i = self.orgs.len() as u32;
        self.orgs.push(name.to_string());
        self.index.insert(name.to_string(), i);
        i
    }
}

fn country(code: &str) -> [u8; 2] {
    let code = code.trim().as_bytes();
    if code.len() == 2 && code.iter().all(u8::is_ascii_alphabetic) && !code.eq_ignore_ascii_case(b"ZZ") {
        [code[0].to_ascii_uppercase(), code[1].to_ascii_uppercase()]
    } else {
        [0, 0]
    }
}

/// Minimal CSV split with quoted values (DB-IP organization names have commas).
fn csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    out.push(current);
    out
}

fn push(layer: &mut Layer, start: &str, end: &str, info: Info) -> bool {
    match (start.trim().parse::<IpAddr>(), end.trim().parse::<IpAddr>()) {
        (Ok(IpAddr::V4(a)), Ok(IpAddr::V4(b))) if u32::from(a) <= u32::from(b) => {
            layer.v4.push((a.into(), b.into(), info));
            true
        }
        (Ok(IpAddr::V6(a)), Ok(IpAddr::V6(b))) if u128::from(a) <= u128::from(b) => {
            layer.v6.push((a.into(), b.into(), info));
            true
        }
        _ => false,
    }
}

fn parse(text: &str, builder: &mut Builder) -> Result<Layer, String> {
    let mut layer = Layer::default();
    let mut rejected = 0usize;
    let mut accepted = 0usize;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let ok = if line.contains('\t') {
            // ip2asn: start, end, AS number, country, AS description. AS 0 = not routed.
            let parts: Vec<&str> = line.split('\t').collect();
            match parts.as_slice() {
                [start, end, asn, cc, rest @ ..] => match asn.trim().parse::<u32>() {
                    Ok(0) => true,
                    Ok(asn) => {
                        let org = builder.org(&rest.join("\t"));
                        push(&mut layer, start, end, Info { asn, country: country(cc), org })
                    }
                    Err(_) => false,
                },
                _ => false,
            }
        } else {
            let parts = csv(line);
            match parts.as_slice() {
                [start, end, cc] => push(&mut layer, start, end, Info { country: country(cc), ..Default::default() }),
                [start, end, asn, org, ..] => match asn.trim().trim_start_matches("AS").parse::<u32>() {
                    Ok(asn) => {
                        let org = builder.org(org);
                        push(&mut layer, start, end, Info { asn, org, ..Default::default() })
                    }
                    Err(_) => false,
                },
                _ => false,
            }
        };
        if ok {
            accepted += 1;
        } else {
            rejected += 1;
            if rejected > 32 && rejected > accepted {
                return Err("O arquivo não está em um formato de base de IPs reconhecido (ip2asn TSV ou DB-IP CSV).".into());
            }
        }
    }
    if layer.v4.is_empty() && layer.v6.is_empty() {
        return Err("Nenhuma faixa de endereços reconhecida no arquivo.".into());
    }
    layer.sort();
    Ok(layer)
}

fn read_text(path: &Path) -> Result<String, String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let head = reader.fill_buf().map_err(|e| e.to_string())?;
    let gzip = head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b;
    let mut bytes = Vec::new();
    if gzip {
        flate2::read::GzDecoder::new(reader).take(MAX_FILE_BYTES).read_to_end(&mut bytes)
    } else {
        reader.take(MAX_FILE_BYTES).read_to_end(&mut bytes)
    }
    .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

pub fn directory() -> PathBuf {
    crate::config_dir().join("geoip")
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
            p.is_file() && !name.ends_with(".part") && [".tsv", ".csv", ".gz", ".txt"].iter().any(|e| name.ends_with(e))
        })
        .collect();
    out.sort();
    out
}

fn load(dir: &Path) -> Result<Option<Database>, String> {
    use sha2::{Digest, Sha256};
    let paths = files(dir);
    if paths.is_empty() {
        return Ok(None);
    }
    let mut builder = Builder::default();
    let mut layers = Vec::new();
    let mut infos = Vec::new();
    let mut hash = Sha256::new();
    hash.update(b"geoip-1");
    for path in paths {
        let text = read_text(&path)?;
        hash.update(path.file_name().and_then(|n| n.to_str()).unwrap_or("").as_bytes());
        hash.update((text.len() as u64).to_le_bytes());
        hash.update(text.as_bytes());
        let layer = parse(&text, &mut builder).map_err(|e| format!("{}: {e}", path.display()))?;
        let meta = std::fs::metadata(&path).ok();
        infos.push(FileInfo {
            name: path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
            bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            ranges: layer.v4.len() + layer.v6.len(),
            modified_ms: meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        });
        layers.push(layer);
    }
    Ok(Some(Database { layers, orgs: builder.orgs, signature: format!("{:x}", hash.finalize()), files: infos }))
}

static ACTIVE: RwLock<Option<Option<Arc<Database>>>> = RwLock::new(None);

/// The loaded base, read once and kept until a base is imported or removed.
pub fn database() -> Option<Arc<Database>> {
    if let Some(loaded) = ACTIVE.read().as_ref() {
        return loaded.clone();
    }
    let mut slot = ACTIVE.write();
    if slot.is_none() {
        #[cfg(test)]
        let loaded = None;
        #[cfg(not(test))]
        let loaded = load(&directory()).ok().flatten().map(Arc::new);
        *slot = Some(loaded);
    }
    slot.as_ref().unwrap().clone()
}

/// Identity of the base; empty without a base, so existing results stay valid.
pub fn signature() -> String {
    database().map(|d| d.signature.clone()).unwrap_or_default()
}

fn reload() -> Result<Option<Arc<Database>>, String> {
    let loaded = load(&directory())?.map(Arc::new);
    *ACTIVE.write() = Some(loaded.clone());
    Ok(loaded)
}

#[cfg(test)]
pub(crate) fn with_test_database<T>(text: &str, f: impl FnOnce() -> T) -> T {
    let mut builder = Builder::default();
    let layer = parse(text, &mut builder).unwrap();
    let db = Database { layers: vec![layer], orgs: builder.orgs, signature: "test".into(), files: vec![] };
    let previous = ACTIVE.write().replace(Some(Arc::new(db)));
    let result = f();
    *ACTIVE.write() = previous;
    result
}

/// Public addresses only; private, loopback and documentation ranges have no location.
fn routable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() || v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64),
        IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80),
    }
}

impl Database {
    pub fn locate(&self, ip: IpAddr) -> Option<Location> {
        if !routable(ip) {
            return None;
        }
        let mut found = Location { country: None, asn: None, org: None, hosting: false };
        for layer in &self.layers {
            if let Some(info) = layer.find(ip) {
                if found.country.is_none() && info.country != [0, 0] {
                    found.country = Some(String::from_utf8_lossy(&info.country).into_owned());
                }
                if found.asn.is_none() && info.asn != 0 {
                    found.asn = Some(info.asn);
                    found.org = self.orgs.get(info.org as usize).filter(|s| !s.is_empty()).cloned();
                }
            }
            if found.country.is_some() && found.asn.is_some() {
                break;
            }
        }
        if found.country.is_none() && found.asn.is_none() {
            return None;
        }
        found.hosting = found.asn.is_some_and(|asn| hosting(asn, found.org.as_deref().unwrap_or("")));
        Some(found)
    }
}

pub fn locate(ip: &str) -> Option<Location> {
    let ip: IpAddr = ip.trim().trim_start_matches('[').trim_end_matches(']').parse().ok()?;
    database()?.locate(ip)
}

/// Autonomous systems of cloud, VPS and hosting providers (not access networks).
const HOSTING_ASNS: &[u32] = &[
    16509, 14618, 8987, // Amazon
    15169, 396982, 19527, 36492, // Google / Google Cloud
    8075, 8068, // Microsoft / Azure
    14061, // DigitalOcean
    16276, 35540, // OVH
    24940, 213230, // Hetzner
    63949, // Linode / Akamai cloud
    20473, // Vultr / Choopa
    51167, 141995, // Contabo
    45102, 37963, // Alibaba
    132203, 45090, // Tencent
    31898, // Oracle Cloud
    12876, // Scaleway
    9009, // M247
    60781, 28753, 59253, 7203, 395954, // LeaseWeb
    36352, // ColoCrossing
    53667, // FranTech / BuyVM
    212238, 60068, // Datacamp / CDN77
    202425, // IP Volume
    49981, // WorldStream
    62240, // Clouvider
    136907, 55990, // Huawei Cloud
    8100, // QuadraNet
    46606, // Unified Layer
    26496, 398101, // GoDaddy
    197540, 8560, // netcup / IONOS
    51396, // Pfcloud
    44477, // Stark Industries
    210644, 209588, // Aeza / Flyservers
    29802, // HVC
    40021, // Contabo US / NL
    3214, // xTom
    32613, // iWeb
    23470, // ReliableSite
    25369, // Hydra
    42831, // UK Dedicated Servers
    396356, // Latitude.sh
    399629, // BL Networks
    61317, // Digital Energy / Heficed
    211252, // Delis
    206092, // IPXO
    401116, // Nybula
    54290, // Hostwinds
    55933, // Cloudie
    18779, // EGIHosting
    19318, // Interserver
    30633, // Leaseweb USA
    33387, // DataShack
    35916, // MULTA-ASN
    36007, // Kamatera
    398324, // Censys
];
const HOSTING_WORDS: &[&str] = &[
    "hosting", "hostinger", "host europe", "datacenter", "data center", "data-center", "server", "vps", "cloud",
    "colocation", "colo ", "dedicated", "amazon", "aws", "google llc", "microsoft", "azure", "digitalocean", "ovh",
    "hetzner", "linode", "vultr", "contabo", "alibaba", "tencent", "oracle", "scaleway", "leaseweb", "m247",
];

pub fn hosting(asn: u32, org: &str) -> bool {
    if HOSTING_ASNS.contains(&asn) {
        return true;
    }
    let org = org.to_ascii_lowercase();
    HOSTING_WORDS.iter().any(|w| org.contains(w))
}

/// Approximate coordinates (capital or population center) per ISO country code.
/// Used only to bound travel speed between two logins; never shown as a location.
const COUNTRIES: &[(&str, f64, f64)] = &[
    ("AD", 42.5, 1.5), ("AE", 24.5, 54.4), ("AF", 34.5, 69.2), ("AG", 17.1, -61.8), ("AL", 41.3, 19.8),
    ("AM", 40.2, 44.5), ("AO", -8.8, 13.2), ("AR", -34.6, -58.4), ("AT", 48.2, 16.4), ("AU", -33.9, 151.2),
    ("AZ", 40.4, 49.9), ("BA", 43.9, 18.4), ("BB", 13.1, -59.6), ("BD", 23.8, 90.4), ("BE", 50.8, 4.4),
    ("BF", 12.4, -1.5), ("BG", 42.7, 23.3), ("BH", 26.2, 50.6), ("BI", -3.4, 29.4), ("BJ", 6.5, 2.6),
    ("BN", 4.9, 114.9), ("BO", -16.5, -68.1), ("BR", -15.8, -47.9), ("BS", 25.1, -77.3), ("BT", 27.5, 89.6),
    ("BW", -24.7, 25.9), ("BY", 53.9, 27.6), ("BZ", 17.3, -88.8), ("CA", 45.4, -75.7), ("CD", -4.3, 15.3),
    ("CF", 4.4, 18.6), ("CG", -4.3, 15.2), ("CH", 46.9, 7.4), ("CI", 5.3, -4.0), ("CL", -33.4, -70.6),
    ("CM", 3.9, 11.5), ("CN", 39.9, 116.4), ("CO", 4.7, -74.1), ("CR", 9.9, -84.1), ("CU", 23.1, -82.4),
    ("CV", 14.9, -23.5), ("CY", 35.2, 33.4), ("CZ", 50.1, 14.4), ("DE", 52.5, 13.4), ("DJ", 11.6, 43.1),
    ("DK", 55.7, 12.6), ("DM", 15.3, -61.4), ("DO", 18.5, -69.9), ("DZ", 36.8, 3.1), ("EC", -0.2, -78.5),
    ("EE", 59.4, 24.8), ("EG", 30.0, 31.2), ("ER", 15.3, 38.9), ("ES", 40.4, -3.7), ("ET", 9.0, 38.7),
    ("FI", 60.2, 24.9), ("FJ", -18.1, 178.4), ("FM", 6.9, 158.2), ("FR", 48.9, 2.4), ("GA", 0.4, 9.5),
    ("GB", 51.5, -0.1), ("GD", 12.1, -61.7), ("GE", 41.7, 44.8), ("GH", 5.6, -0.2), ("GM", 13.5, -16.6),
    ("GN", 9.6, -13.6), ("GQ", 3.8, 8.8), ("GR", 38.0, 23.7), ("GT", 14.6, -90.5), ("GW", 11.9, -15.6),
    ("GY", 6.8, -58.2), ("HK", 22.3, 114.2), ("HN", 14.1, -87.2), ("HR", 45.8, 16.0), ("HT", 18.5, -72.3),
    ("HU", 47.5, 19.0), ("ID", -6.2, 106.8), ("IE", 53.3, -6.3), ("IL", 31.8, 35.2), ("IN", 28.6, 77.2),
    ("IQ", 33.3, 44.4), ("IR", 35.7, 51.4), ("IS", 64.1, -21.9), ("IT", 41.9, 12.5), ("JM", 18.0, -76.8),
    ("JO", 32.0, 35.9), ("JP", 35.7, 139.7), ("KE", -1.3, 36.8), ("KG", 42.9, 74.6), ("KH", 11.6, 104.9),
    ("KI", 1.3, 173.0), ("KM", -11.7, 43.3), ("KN", 17.3, -62.7), ("KP", 39.0, 125.8), ("KR", 37.6, 127.0),
    ("KW", 29.4, 48.0), ("KZ", 51.2, 71.4), ("LA", 18.0, 102.6), ("LB", 33.9, 35.5), ("LC", 14.0, -61.0),
    ("LI", 47.1, 9.5), ("LK", 6.9, 79.9), ("LR", 6.3, -10.8), ("LS", -29.3, 27.5), ("LT", 54.7, 25.3),
    ("LU", 49.6, 6.1), ("LV", 56.9, 24.1), ("LY", 32.9, 13.2), ("MA", 34.0, -6.8), ("MC", 43.7, 7.4),
    ("MD", 47.0, 28.9), ("ME", 42.4, 19.3), ("MG", -18.9, 47.5), ("MH", 7.1, 171.4), ("MK", 42.0, 21.4),
    ("ML", 12.6, -8.0), ("MM", 19.8, 96.1), ("MN", 47.9, 106.9), ("MO", 22.2, 113.5), ("MR", 18.1, -16.0),
    ("MT", 35.9, 14.5), ("MU", -20.2, 57.5), ("MV", 4.2, 73.5), ("MW", -14.0, 33.8), ("MX", 19.4, -99.1),
    ("MY", 3.1, 101.7), ("MZ", -26.0, 32.6), ("NA", -22.6, 17.1), ("NE", 13.5, 2.1), ("NG", 9.1, 7.5),
    ("NI", 12.1, -86.3), ("NL", 52.4, 4.9), ("NO", 59.9, 10.8), ("NP", 27.7, 85.3), ("NR", -0.5, 166.9),
    ("NZ", -41.3, 174.8), ("OM", 23.6, 58.4), ("PA", 9.0, -79.5), ("PE", -12.0, -77.0), ("PG", -9.4, 147.2),
    ("PH", 14.6, 121.0), ("PK", 33.7, 73.1), ("PL", 52.2, 21.0), ("PR", 18.5, -66.1), ("PS", 31.9, 35.2),
    ("PT", 38.7, -9.1), ("PW", 7.5, 134.6), ("PY", -25.3, -57.6), ("QA", 25.3, 51.5), ("RO", 44.4, 26.1),
    ("RS", 44.8, 20.5), ("RU", 55.8, 37.6), ("RW", -1.9, 30.1), ("SA", 24.7, 46.7), ("SB", -9.4, 160.0),
    ("SC", -4.6, 55.5), ("SD", 15.6, 32.5), ("SE", 59.3, 18.1), ("SG", 1.3, 103.8), ("SI", 46.1, 14.5),
    ("SK", 48.1, 17.1), ("SL", 8.5, -13.2), ("SM", 43.9, 12.4), ("SN", 14.7, -17.4), ("SO", 2.0, 45.3),
    ("SR", 5.9, -55.2), ("SS", 4.9, 31.6), ("ST", 0.3, 6.7), ("SV", 13.7, -89.2), ("SY", 33.5, 36.3),
    ("SZ", -26.3, 31.1), ("TD", 12.1, 15.0), ("TG", 6.1, 1.2), ("TH", 13.8, 100.5), ("TJ", 38.6, 68.8),
    ("TL", -8.6, 125.6), ("TM", 37.9, 58.4), ("TN", 36.8, 10.2), ("TO", -21.1, -175.2), ("TR", 39.9, 32.9),
    ("TT", 10.7, -61.5), ("TV", -8.5, 179.2), ("TW", 25.0, 121.6), ("TZ", -6.8, 39.3), ("UA", 50.5, 30.5),
    ("UG", 0.3, 32.6), ("US", 39.8, -98.6), ("UY", -34.9, -56.2), ("UZ", 41.3, 69.3), ("VA", 41.9, 12.5),
    ("VC", 13.2, -61.2), ("VE", 10.5, -66.9), ("VN", 21.0, 105.8), ("VU", -17.7, 168.3), ("WS", -13.8, -171.8),
    ("YE", 15.4, 44.2), ("ZA", -26.2, 28.0), ("ZM", -15.4, 28.3), ("ZW", -17.8, 31.0), ("RE", -20.9, 55.5),
    ("GF", 4.9, -52.3), ("GP", 16.2, -61.5), ("MQ", 14.6, -61.1), ("NC", -22.3, 166.5), ("PF", -17.5, -149.6),
    ("GL", 64.2, -51.7), ("FO", 62.0, -6.8), ("GI", 36.1, -5.4), ("IM", 54.2, -4.5), ("JE", 49.2, -2.1),
    ("GG", 49.5, -2.5), ("BM", 32.3, -64.8), ("KY", 19.3, -81.4), ("VG", 18.4, -64.6), ("VI", 18.3, -64.9),
    ("AW", 12.5, -70.0), ("CW", 12.1, -68.9), ("GU", 13.4, 144.8), ("XK", 42.7, 21.2),
];

/// Countries whose territory is too large for one point to bound travel time.
const WIDE_COUNTRIES: &[&str] = &["US", "RU", "CA", "BR", "AU", "CN", "IN", "AR", "KZ", "ID", "MX", "CL"];

pub fn coordinates(code: &str) -> Option<(f64, f64)> {
    COUNTRIES.iter().find(|(c, _, _)| c.eq_ignore_ascii_case(code)).map(|&(_, lat, lon)| (lat, lon))
}

/// Great-circle distance in kilometres.
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lon1) = (a.0.to_radians(), a.1.to_radians());
    let (lat2, lon2) = (b.0.to_radians(), b.1.to_radians());
    let h = ((lat2 - lat1) / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * ((lon2 - lon1) / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}

/// Lower bound of the distance between two countries: their reference points minus
/// a margin for large territories, so a long border never looks like a long trip.
pub fn minimum_distance_km(a: &str, b: &str) -> Option<f64> {
    if a.eq_ignore_ascii_case(b) {
        return Some(0.0);
    }
    let margin = |c: &str| if WIDE_COUNTRIES.iter().any(|w| w.eq_ignore_ascii_case(c)) { 1500.0 } else { 300.0 };
    let d = distance_km(coordinates(a)?, coordinates(b)?) - margin(a) - margin(b);
    Some(d.max(0.0))
}

// ------------------------------------------------------------------ commands

#[derive(Serialize)]
pub struct Status {
    pub loaded: bool,
    pub files: Vec<FileInfo>,
    pub directory: String,
    pub download_url: &'static str,
    pub error: Option<String>,
}

fn status_now(error: Option<String>) -> Status {
    let db = database();
    Status {
        loaded: db.is_some(),
        files: db.map(|d| d.files.clone()).unwrap_or_default(),
        directory: directory().display().to_string(),
        download_url: DOWNLOAD_URL,
        error,
    }
}

fn install(source: &Path, name: &str) -> Result<(), String> {
    let dir = directory();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut builder = Builder::default();
    parse(&read_text(source)?, &mut builder)?;
    let target = dir.join(name);
    let partial = dir.join(format!("{name}.part"));
    std::fs::copy(source, &partial).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &target).map_err(|e| e.to_string())?;
    reload()?;
    Ok(())
}

fn safe_name(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("base.tsv");
    let cleaned: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_".contains(c) { c } else { '_' }).collect();
    if [".tsv", ".csv", ".gz", ".txt"].iter().any(|e| cleaned.to_ascii_lowercase().ends_with(e)) { cleaned } else { format!("{cleaned}.txt") }
}

#[tauri::command]
pub async fn geoip_status() -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let error = reload().err();
        status_now(error)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn geoip_import(path: String) -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source = PathBuf::from(&path);
        let meta = std::fs::metadata(&source).map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            return Err("Selecione um arquivo de base de IPs com até 512 MB.".to_string());
        }
        install(&source, &safe_name(&source))?;
        Ok(status_now(None))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Downloads the public-domain iptoasn.com base once, on the user's request.
#[tauri::command]
pub async fn geoip_download() -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let dir = directory();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .user_agent("LogInsight")
            .build()
            .map_err(|e| e.to_string())?;
        let response = client.get(DOWNLOAD_URL).send().map_err(|e| format!("Download da base: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("Download da base: HTTP {}", response.status()));
        }
        let temporary = dir.join("download.tmp.part");
        let mut file = std::fs::File::create(&temporary).map_err(|e| e.to_string())?;
        let copied = std::io::copy(&mut response.take(MAX_FILE_BYTES), &mut file).map_err(|e| e.to_string())?;
        drop(file);
        if copied == 0 {
            let _ = std::fs::remove_file(&temporary);
            return Err("Download da base: arquivo vazio".into());
        }
        let result = install(&temporary, "ip2asn-combined.tsv.gz");
        let _ = std::fs::remove_file(&temporary);
        result?;
        Ok(status_now(None))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn geoip_remove() -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(|| {
        for path in files(&directory()) {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        reload()?;
        Ok(status_now(None))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n\
        8.8.8.0\t8.8.8.255\t15169\tUS\tGOOGLE\n\
        177.0.0.0\t177.0.255.255\t28573\tBR\tClaro NXT Telecomunicacoes Ltda\n\
        185.0.0.0\t185.0.0.255\t0\tNone\tNot routed\n\
        2001:db8::\t2001:db8:ffff:ffff:ffff:ffff:ffff:ffff\t64500\tPT\tExample\n";

    #[test]
    fn ip2asn_lookup_returns_country_asn_and_hosting() {
        let mut builder = Builder::default();
        let layer = parse(SAMPLE, &mut builder).unwrap();
        let db = Database { layers: vec![layer], orgs: builder.orgs, signature: "s".into(), files: vec![] };
        let google = db.locate("8.8.8.8".parse().unwrap()).unwrap();
        assert_eq!(google.country.as_deref(), Some("US"));
        assert_eq!(google.asn, Some(15169));
        assert!(google.hosting);
        let claro = db.locate("177.0.12.1".parse().unwrap()).unwrap();
        assert_eq!(claro.country.as_deref(), Some("BR"));
        assert!(!claro.hosting);
        assert!(db.locate("185.0.0.1".parse().unwrap()).is_none());
        assert!(db.locate("10.1.2.3".parse().unwrap()).is_none());
        assert!(db.locate("9.9.9.9".parse().unwrap()).is_none());
        assert_eq!(db.locate("2001:db8::1".parse().unwrap()).unwrap().country.as_deref(), Some("PT"));
        assert_eq!(db.locate("::ffff:8.8.8.8".parse().unwrap()).unwrap().asn, Some(15169));
    }

    #[test]
    fn dbip_country_and_asn_files_combine() {
        let mut builder = Builder::default();
        let countries = parse("1.2.3.0,1.2.3.255,AU\n", &mut builder).unwrap();
        let asns = parse("1.2.3.0,1.2.3.255,13335,\"Cloudflare, Inc.\"\n", &mut builder).unwrap();
        let db = Database { layers: vec![countries, asns], orgs: builder.orgs, signature: "s".into(), files: vec![] };
        let found = db.locate("1.2.3.4".parse().unwrap()).unwrap();
        assert_eq!(found.country.as_deref(), Some("AU"));
        assert_eq!(found.asn, Some(13335));
        assert_eq!(found.org.as_deref(), Some("Cloudflare, Inc."));
    }

    #[test]
    fn unrelated_text_is_rejected() {
        let text = (0..100).map(|i| format!("linha de log {i}\n")).collect::<String>();
        assert!(parse(&text, &mut Builder::default()).is_err());
    }

    #[test]
    fn travel_distance_ignores_large_territories_and_neighbours() {
        assert_eq!(minimum_distance_km("BR", "BR"), Some(0.0));
        assert!(minimum_distance_km("PT", "ES").unwrap() < 300.0);
        assert!(minimum_distance_km("BR", "JP").unwrap() > 10000.0);
        assert!(minimum_distance_km("BR", "XX").is_none());
    }
}
