//! Loading logs of many families the way the app does: detected format,
//! a time for every record, and the fields each family carries. Synthetic
//! data only (RFC 5737 addresses, example hosts).
use loginsight_lib::testkit::{self, Event};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn workdir() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("dados"));
        std::env::set_var("LOGINSIGHT_ENGINE", "0");
        dir
    })
    .path()
}

fn load_bytes(name: &str, bytes: &[u8]) -> (String, Vec<Event>) {
    let path: PathBuf = workdir().join(name);
    std::fs::write(&path, bytes).unwrap();
    testkit::load(&path.to_string_lossy()).unwrap()
}

fn load(name: &str, text: &str) -> (String, Vec<Event>) {
    load_bytes(name, text.as_bytes())
}

fn field<'a>(event: &'a Event, key: &str) -> &'a str {
    event.fields.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("{key} missing: {:?}", event.fields))
}

fn all_dated(events: &[Event]) {
    for event in events {
        assert!(event.timestamp.is_some(), "undated: {}", event.raw);
        assert_eq!(event.parse_status, "parsed", "{}", event.raw);
    }
}

fn utc(text: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(text).unwrap().timestamp_millis()
}

fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    let naive = chrono::NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, s).unwrap();
    naive.and_local_timezone(chrono::Local).single().unwrap().timestamp_millis()
}

#[test]
fn tomcat_access_log_without_quotes() {
    let text = "\
192.168.21.40 - - [12/Oct/2026:00:00:00 -0300] GET /controleacesso/img/fechar-pop.jpg HTTP/1.1 200 2789 0 0.000 http-0.0.0.0-8443-86 Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Amazonbot/0.1; +https://developer.amazon.com/support/amazonbot) Chrome/119.0.6045.214 Safari/537.36
192.168.21.40 - - [12/Oct/2026:00:00:00 -0300] GET /srptv/debug.seam?cid=124588&name=org.jboss.seam.faces.redirect.component HTTP/1.1 200 30650 2569 2.569 http-0.0.0.0-8443-38 Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; GPTBot/1.4; +https://openai.com/gptbot)
192.168.21.40 - - [12/Oct/2026:00:00:01 -0300] GET /favicon.ico HTTP/1.1 302 - 1 0.001 http-0.0.0.0-8443-7 Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36
192.168.21.40 - - [12/Oct/2026:12:01:14 -0300] POST /idp/profile/SAML2/POST/SSO HTTP/1.1 500 1306 15 0.015 http-0.0.0.0-8443-10 -
192.168.21.40 - - [12/Oct/2026:12:01:14 -0300] GET /zabbix/ping HTTP/1.1 200 2 1 0.001 http-0.0.0.0-8443-11 Apache CXF 2.7.18.SP6-redhat-1
192.168.21.40 - - [12/Oct/2026:12:01:14 -0300] GET /srptv/debug.seam?cid=39488&name=org.jboss.seam.faces.uiComponent.
";
    let (format, events) = load("access_log.2026-10-12", text);
    assert_eq!(format, "apache");
    assert_eq!(events.len(), 6);
    assert_eq!(field(&events[3], "agent"), "-");
    assert_eq!(field(&events[4], "agent"), "Apache CXF 2.7.18.SP6-redhat-1");
    assert!(events.iter().all(|e| !e.fields.contains_key("access.extra.1")));
    let first = &events[0];
    assert_eq!(first.timestamp, Some(utc("2026-10-12T03:00:00Z")));
    assert_eq!((field(first, "method"), field(first, "path"), field(first, "status")), ("GET", "/controleacesso/img/fechar-pop.jpg", "200"));
    assert_eq!((field(first, "duration_ms"), field(first, "duration_s"), field(first, "thread")), ("0", "0.000", "http-0.0.0.0-8443-86"));
    assert!(field(first, "agent").starts_with("Mozilla/5.0 AppleWebKit/537.36 (KHTML"));
    assert_eq!(field(first, "client_ip"), "192.168.21.40");
    assert_eq!(field(&events[1], "path.cid"), "124588");
    assert_eq!(field(&events[1], "duration_ms"), "2569");
    assert_eq!(field(&events[2], "size"), "-");
    assert_eq!((events[3].level.as_str(), events[3].code.as_str()), ("Erro", "500"));
    // A record cut in the middle still keeps its time and request.
    assert_eq!(events[5].timestamp, Some(utc("2026-10-12T15:01:14Z")));
    assert_eq!(field(&events[5], "method"), "GET");
    for event in &events {
        assert!(event.timestamp.is_some(), "{}", event.raw);
    }
}

#[test]
fn web_server_variants() {
    let (format, events) = load(
        "vhost.log",
        "www.example.test:443 192.0.2.10 - alice [12/Oct/2026:10:00:00 +0000] \"GET /a HTTP/2.0\" 200 512 \"-\" \"curl/8.4.0\"\n\
         192.0.2.11, 198.51.100.7 10.0.0.2 - - [12/Oct/2026:10:00:01 +0000] \"POST /b HTTP/1.1\" 201 12 \"https://example.test/\" \"Mozilla/5.0\" 0.012 0.010 [backend] 5ms\n\
         10.0.0.3 - - [2026-10-12T10:00:02+00:00] \"GET /c HTTP/1.1\" 404 0\n",
    );
    assert_eq!(format, "apache");
    all_dated(&events);
    assert_eq!((field(&events[0], "vhost"), field(&events[0], "user"), field(&events[0], "agent")), ("www.example.test:443", "alice", "curl/8.4.0"));
    assert_eq!(field(&events[1], "forwarded_for"), "192.0.2.11, 198.51.100.7");
    assert_eq!((field(&events[1], "access.extra.1"), field(&events[1], "duration")), ("0.012", "5ms"));
    assert_eq!(events[2].timestamp, Some(utc("2026-10-12T10:00:02Z")));

    let (format, events) = load(
        "error_log",
        "[Mon Oct 12 00:00:00.123456 2026] [core:error] [pid 1234:tid 5678] [client 192.0.2.1:5000] AH00126: Invalid URI in request GET /x HTTP/1.1\n\
         [Mon Oct 12 00:00:01 2026] [mpm_event:notice] [pid 1:tid 2] AH00489: Apache/2.4.58 configured -- resuming normal operations\n\
         [Mon Oct 12 00:00:02.000001 2026] [ssl:warn] [pid 1234] AH01909: server certificate does NOT include an ID which matches the server name\n",
    );
    assert_eq!(format, "apache-error");
    all_dated(&events);
    assert_eq!((events[0].level.as_str(), field(&events[0], "client_ip"), events[0].code.as_str()), ("Erro", "192.0.2.1", "AH00126"));
    assert_eq!(events[2].level, "Aviso");
}

#[test]
fn syslog_with_any_timestamp_layout() {
    let (format, events) = load(
        "messages",
        "2026-10-12T00:00:00.123456-03:00 web01 sshd[1234]: Failed password for invalid user admin from 192.0.2.7 port 4242 ssh2\n\
         2026-10-12T00:00:01.000000-03:00 web01 sudo[99]: alice : TTY=pts/0 ; PWD=/home/alice ; USER=root ; COMMAND=/usr/bin/id\n\
         2026-10-12T00:00:02.000000-03:00 web01 kernel: [UFW BLOCK] IN=eth0 OUT= SRC=203.0.113.9 DST=10.0.0.2 PROTO=TCP SPT=4444 DPT=22\n",
    );
    assert_eq!(format, "syslog3164");
    all_dated(&events);
    assert_eq!(events[0].timestamp, Some(utc("2026-10-12T03:00:00.123Z")));
    assert_eq!((events[0].source.as_str(), field(&events[0], "process"), field(&events[0], "pid")), ("web01", "sshd", "1234"));
    assert_eq!(field(&events[1], "COMMAND"), "/usr/bin/id");
    assert_eq!(field(&events[2], "SRC"), "203.0.113.9");

    // Year-less lines take the current year, whatever date the file name carries
    // (a name may hold the first, the last or any day of the log).
    let current = chrono::Local::now().format("%Y").to_string();
    let year = |e: &Event| chrono::DateTime::<chrono::Local>::from(chrono::DateTime::from_timestamp_millis(e.timestamp.unwrap()).unwrap()).format("%Y").to_string();
    let (_, events) = load("secure-20261012", "Dec 30 23:59:59 host sshd[1]: old\nOct 12 08:00:00 host sshd[1]: new\n");
    assert_eq!((year(&events[0]), year(&events[1])), (current.clone(), current.clone()));
    let (_, events) = load("rsyslog-gerado-indevidamente-20260616.log", "Jun 16 06:30:01 rj-jb5 CROND[1]: (jboss) CMD (x)\nJun 20 06:30:01 rj-jb5 CROND[2]: (jboss) CMD (x)\n");
    assert!(events.iter().all(|e| year(e) == current), "{:?}", events.iter().map(|e| e.timestamp).collect::<Vec<_>>());
    assert!(events[0].timestamp < events[1].timestamp);

    let (format, events) = load(
        "cef.log",
        "<134>Oct 12 10:00:00 fw01 CEF:0|Vendor|Firewall|1.0|100|Blocked connection|8|src=192.0.2.1 dst=10.0.0.1 dpt=443\n\
         <134>Oct 12 10:00:01 fw01 CEF:0|Vendor|Firewall|1.0|101|Allowed connection|3|src=192.0.2.2 dst=10.0.0.1 dpt=80\n",
    );
    assert_eq!(format, "cef");
    all_dated(&events);
    assert_eq!((events[0].code.as_str(), field(&events[0], "src"), field(&events[0], "host")), ("100", "192.0.2.1", "fw01"));
}

#[test]
fn application_logs_with_multi_line_traces() {
    let text = "\
2026-10-12 00:00:00.123  INFO 1234 --- [           main] c.e.demo.DemoApplication                 : Started DemoApplication in 3.2 seconds
2026-10-12 00:00:05.456 ERROR 1234 --- [nio-8080-exec-1] o.a.c.c.C.[.[.[/].[dispatcherServlet]    : Servlet.service() threw exception
java.lang.IllegalStateException: Pedido 42 inválido
\tat com.example.demo.Service.handle(Service.java:10)
\tat com.example.demo.Controller.get(Controller.java:20)
Caused by: java.lang.NumberFormatException: For input string: \"x\"
\t... 2 more
2026-10-12 00:00:06.000  WARN 1234 --- [nio-8080-exec-2] c.e.demo.Controller                      : Slow request user=bob elapsed_ms=1200
";
    let (format, events) = load("spring.log", text);
    assert_eq!(format, "mixed");
    assert_eq!(events.len(), 3, "{:#?}", events.iter().map(|e| &e.raw).collect::<Vec<_>>());
    all_dated(&events);
    assert_eq!((events[1].level.as_str(), field(&events[1], "thread")), ("Erro", "nio-8080-exec-1"));
    assert_eq!(field(&events[1], "exception"), "java.lang.IllegalStateException: Pedido 42 inválido");
    assert_eq!((events[2].level.as_str(), field(&events[2], "user"), field(&events[2], "elapsed_ms")), ("Aviso", "bob", "1200"));

    let text = "\
2026-10-12 00:00:00,001 - worker.tasks - INFO - Job 7 started
2026-10-12 00:00:01,002 - worker.tasks - ERROR - Job 7 failed
Traceback (most recent call last):
  File \"/app/tasks.py\", line 10, in run
    value = int(raw)
ValueError: invalid literal for int() with base 10: 'x'
2026-10-12 00:00:02,003 - worker.tasks - INFO - Job 8 started
";
    let (format, events) = load("worker.log", text);
    assert_eq!(format, "mixed");
    assert_eq!(events.len(), 3);
    assert_eq!((events[1].level.as_str(), field(&events[1], "logger")), ("Erro", "worker.tasks"));
    assert_eq!(field(&events[1], "exception"), "ValueError: invalid literal for int() with base 10: 'x'");
}

#[test]
fn servers_databases_and_platforms() {
    for (name, text, checks) in [
        (
            "elasticsearch.log",
            "[2026-10-12T00:00:00,123][INFO ][o.e.n.Node               ] [node-1] started\n[2026-10-12T00:00:01,000][WARN ][o.e.c.r.a.DiskThresholdMonitor] [node-1] high disk watermark exceeded\n",
            vec![("logger", "o.e.n.Node")],
        ),
        (
            "postgresql.log",
            "2026-10-12 00:00:00.123 -03 [4321] LOG:  database system is ready to accept connections\n2026-10-12 00:00:01.000 -03 [4322] ERROR:  relation \"x\" does not exist\n",
            vec![("pid", "4321")],
        ),
        (
            "mysqld.log",
            "2026-10-12T03:00:00.123456Z 0 [System] [MY-010116] [Server] /usr/sbin/mysqld starting as process 1\n2026-10-12T03:00:01.000000Z 8 [Warning] [MY-010055] [Server] IP address could not be resolved\n",
            vec![],
        ),
        (
            "kubelet.log",
            "I1012 00:00:00.123456    4321 kubelet.go:2213] \"SyncLoop ADD\" source=\"api\"\nE1012 00:00:01.000000    4321 pod_workers.go:1300] \"Error syncing pod\" err=\"failed\"\n",
            vec![("caller", "kubelet.go:2213")],
        ),
        (
            "containers.log",
            "2026-10-12T03:00:00.123456789Z stdout F {\"level\":\"info\",\"msg\":\"listening\",\"port\":8080}\n2026-10-12T03:00:01.000000000Z stderr F {\"level\":\"error\",\"msg\":\"boom\"}\n",
            vec![("stream", "stdout"), ("port", "8080")],
        ),
        (
            "access.log",
            "1286536309.586    921 192.0.2.68 TCP_MISS/200 507 POST http://www.example.test/x - DIRECT/198.51.100.4 text/html\n1286536310.000     12 192.0.2.69 TCP_HIT/304 0 GET http://www.example.test/y - NONE/- -\n",
            vec![("cache_result", "TCP_MISS")],
        ),
        (
            "dns.log",
            "10/12/2026 12:00:00 AM 0A3C PACKET  000000D3C6A0 UDP Rcv 192.0.2.1       0002   Q [0001   D   NOERROR] A      (3)www(7)example(4)test(0)\n10/12/2026 1:15:30 PM 0A3C PACKET  000000D3C6A0 UDP Snd 192.0.2.1       0002 R Q [8081   DR  NOERROR] A      (3)www(7)example(4)test(0)\n",
            vec![],
        ),
        (
            "catalina.out",
            "12-Oct-2026 00:00:00.123 INFO [main] org.apache.catalina.startup.Catalina.start Server startup in [1234] milliseconds\n12-Oct-2026 00:00:01.000 SEVERE [main] org.apache.catalina.core.StandardContext.startInternal One or more listeners failed\n",
            vec![("thread", "main")],
        ),
        (
            "fortigate.log",
            "date=2026-10-12 time=00:00:00 devname=\"FG100\" logid=\"0000000013\" type=\"traffic\" level=\"notice\" srcip=192.0.2.5 dstip=198.51.100.9 action=\"deny\"\ndate=2026-10-12 time=00:00:01 devname=\"FG100\" logid=\"0000000013\" type=\"traffic\" level=\"warning\" srcip=192.0.2.6 dstip=198.51.100.9 action=\"deny\"\n",
            vec![("srcip", "192.0.2.5")],
        ),
    ] {
        let (format, events) = load(name, text);
        all_dated(&events);
        for (key, value) in checks {
            let actual = match &events[0].fields.get(key) {
                Some(Value::String(text)) => text.clone(),
                Some(other) => other.to_string(),
                None => panic!("{name} ({format}): {key} missing in {:?}", events[0].fields),
            };
            assert_eq!(actual, value, "{name} ({format})");
        }
    }
}

#[test]
fn delimited_logs_with_and_without_headers() {
    let (format, events) = load(
        "panos.csv",
        "1,2026/10/12 00:00:00,012345678901,TRAFFIC,end,2561,2026/10/12 00:00:00,192.0.2.1,198.51.100.1,allow\n\
         1,2026/10/12 00:00:01,012345678901,TRAFFIC,end,2561,2026/10/12 00:00:01,192.0.2.2,198.51.100.1,deny\n\
         1,2026/10/12 00:00:02,012345678901,THREAT,url,2561,2026/10/12 00:00:02,192.0.2.3,198.51.100.1,alert\n",
    );
    assert_eq!(format, "csv-positional");
    all_dated(&events);
    assert_eq!(field(&events[1], "coluna 4"), "TRAFFIC");

    let dhcp = "\t\tMicrosoft DHCP Service Activity Log\n\nEvent ID  Meaning\n00\tThe log was started.\n\n\
ID,Date,Time,Description,IP Address,Host Name,MAC Address\n\
10,10/12/26,00:00:00,Assign,192.0.2.20,pc01.example.test,AABBCCDDEEFF\n\
11,10/12/26,00:05:00,Renew,192.0.2.21,pc02.example.test,AABBCCDDEE00\n\
11,10/13/26,00:10:00,Renew,192.0.2.22,pc03.example.test,AABBCCDDEE01\n";
    let (format, events) = load("DhcpSrvLog-Mon.log", dhcp);
    assert_eq!(format, "csv");
    let rows: Vec<&Event> = events.iter().filter(|e| e.fields.contains_key("MAC Address")).collect();
    assert_eq!(rows.len(), 3);
    // 10/13/26 shows the file writes the month first, so 10/12/26 is October 12.
    assert_eq!(rows[0].timestamp, Some(local(2026, 10, 12, 0, 0, 0)));
    assert_eq!(rows[0].source, "pc01.example.test");

    let (format, events) = load(
        "cloudfront.log",
        "#Version: 1.0\n#Fields: date time x-edge-location sc-bytes c-ip cs-method cs(Host) cs-uri-stem sc-status\n\
         2026-10-12\t03:00:00\tGRU1\t512\t192.0.2.9\tGET\td1.cloudfront.example\t/index.html\t200\n",
    );
    assert_eq!(format, "w3c");
    assert_eq!(events[0].timestamp, Some(utc("2026-10-12T03:00:00Z")), "CloudFront writes UTC");
    assert_eq!(field(&events[0], "cs-uri-stem"), "/index.html");
}

#[test]
fn time_only_records_take_the_date_of_the_file() {
    let (format, events) = load(
        "server.log.2026-06-24",
        "00:00:00,001 INFO  [org.jboss.as] (MSC service thread 1-2) WFLYSRV0049: started\n23:59:59,999 ERROR [a.b.C] (default task-1) failed\n",
    );
    assert_eq!(format, "wildfly");
    let local = |h, m, s, ms| {
        let naive = chrono::NaiveDate::from_ymd_opt(2026, 6, 24).unwrap().and_hms_milli_opt(h, m, s, ms).unwrap();
        naive.and_local_timezone(chrono::Local).single().unwrap().timestamp_millis()
    };
    assert_eq!(events[0].timestamp, Some(local(0, 0, 0, 1)));
    assert_eq!(events[1].timestamp, Some(local(23, 59, 59, 999)));
}

#[test]
fn any_text_gets_its_time_and_structure() {
    let (_, events) = load(
        "app.txt",
        "Started at 2026-10-12 10:00:00 by scheduler\n\
         [WARN] queue depth=120 limit=100\n\
         \u{1b}[32m2026-10-12 10:00:02\u{1b}[0m \u{1b}[31mERROR\u{1b}[0m database unreachable host=db01\n\
         plain line without anything\n",
    );
    assert!(events[0].timestamp.is_some());
    assert_eq!((events[1].level.as_str(), field(&events[1], "depth")), ("Aviso", "120"));
    assert_eq!((events[2].level.as_str(), events[2].message.as_str()), ("Erro", "database unreachable host=db01"));
    assert_eq!(events[3].parse_status, "unparsed");
}

#[test]
fn encodings_compression_and_line_endings() {
    let text = "2026-10-12 10:00:00 ERROR Falha de autenticação do usuário José\r\n2026-10-12 10:00:01 INFO Conexão encerrada\r\n";
    let utf16: Vec<u8> = [0xff, 0xfe].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
    let (_, events) = load_bytes("utf16.log", &utf16);
    assert_eq!((events.len(), events[0].level.as_str()), (2, "Erro"));
    assert!(events[0].message.contains("usuário José"));

    let utf32: Vec<u8> = text.chars().flat_map(|c| (c as u32).to_le_bytes()).collect();
    let (_, events) = load_bytes("utf32.log", &utf32);
    assert!(events[1].message.contains("Conexão"));

    let latin1 = encoding_rs::WINDOWS_1252.encode(text).0.into_owned();
    let (_, events) = load_bytes("latin1.log", &latin1);
    assert!(events[0].message.contains("autenticação"));

    // A UTF-8 file with one line written in a legacy code page.
    let mut mixed = "2026-10-12 10:00:00 INFO ação válida\n".as_bytes().to_vec();
    mixed.extend_from_slice(b"2026-10-12 10:00:01 INFO a\xe7\xe3o legada\n");
    let (_, events) = load_bytes("mixed-encoding.log", &mixed);
    assert_eq!(events[1].message, "ação legada");

    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gzip, text.as_bytes()).unwrap();
    let (_, events) = load_bytes("app.log.1", &gzip.finish().unwrap());
    assert_eq!(events.len(), 2);
    all_dated(&events);

    let (_, events) = load("classic-mac.log", "2026-10-12 10:00:00 INFO one\r2026-10-12 10:00:01 INFO two\r2026-10-12 10:00:02 INFO three\r");
    assert_eq!(events.len(), 3);

    let mut padded = text.as_bytes().to_vec();
    padded.extend(std::iter::repeat_n(0u8, 4096));
    let (_, events) = load_bytes("crashed.log", &padded);
    assert_eq!(events.len(), 2, "NUL padding is not a record");
}

/// Summary of how a real file is read: `LOGINSIGHT_SAMPLE_FILE=<path>`.
#[test]
#[ignore = "Reads a local file named by LOGINSIGHT_SAMPLE_FILE."]
fn profile_local_file() {
    workdir();
    let path = std::env::var("LOGINSIGHT_SAMPLE_FILE").expect("LOGINSIGHT_SAMPLE_FILE");
    let profile = testkit::profile(&path, 12).unwrap();
    println!("{}", serde_json::to_string_pretty(&profile).unwrap());
}

#[test]
#[ignore = "Local sample: LOGINSIGHT_SAMPLE_FILE=path"]
fn clients_of_local_file() {
    workdir();
    let path = std::env::var("LOGINSIGHT_SAMPLE_FILE").expect("LOGINSIGHT_SAMPLE_FILE");
    println!("{}", serde_json::to_string_pretty(&testkit::client_inventory(&path).unwrap()).unwrap());
}

#[test]
fn json_objects_one_after_another_and_interrupted_exports() {
    let text = "{\n  \"@timestamp\": \"2026-10-12T03:00:00Z\",\n  \"level\": \"error\",\n  \"message\": \"first\"\n}\n{\n  \"@timestamp\": \"2026-10-12T03:00:01Z\",\n  \"message\": \"second\",\n  \"user\": {\"name\": \"ana\"}\n}\n";
    let (format, events) = load("pretty.json", text);
    assert_eq!(format, "jsonl");
    assert_eq!(events.len(), 2);
    all_dated(&events);
    assert_eq!((events[0].level.as_str(), events[1].message.as_str(), field(&events[1], "user.name")), ("Erro", "second", "ana"));

    let (_, events) = load("single.json", "{\n  \"time\": \"2026-10-12 00:00:00\",\n  \"msg\": \"only one\"\n}\n");
    assert_eq!((events.len(), events[0].message.as_str()), (1, "only one"));

    // An export cut while writing keeps every complete record.
    let (format, events) = load("cut.json", "[\n{\"timestamp\":\"2026-10-12T03:00:00Z\",\"message\":\"a\"},\n{\"timestamp\":\"2026-10-12T03:00:01Z\",\"message\":\"b\"},\n{\"timestamp\":\"2026-10-12T03:00:02Z\",\"mess");
    assert_eq!(format, "jsonl");
    assert_eq!(events.len(), 3);
    assert_eq!((events[0].message.as_str(), events[1].message.as_str()), ("a", "b"));
}

#[test]
fn more_layouts_from_the_field() {
    // Oracle alert log: a timestamp alone heads the lines below it.
    let (format, events) = load(
        "alert_ORCL.log",
        "2026-10-12T10:00:00.123456-03:00\nStarting ORACLE instance (normal)\n  LICENSE_MAX_SESSION = 0\n2026-10-12T10:00:05.000000-03:00\nORA-00600: internal error code, arguments: [kcbz]\n",
    );
    assert_eq!(format, "mixed");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].message, "Starting ORACLE instance (normal)");
    assert_eq!(events[1].timestamp, Some(utc("2026-10-12T13:00:05Z")));

    // Caddy and zap console: lowercase or uppercase levels in tab-separated columns.
    let (_, events) = load(
        "caddy.log",
        "2026/10/12 10:00:00.000\tinfo\thttp.log\tserver running\t{\"name\": \"srv0\"}\n2026/10/12 10:00:01.000\terror\tmyapp/server.go:42\tlisten failed\t{\"port\": 443}\n",
    );
    assert_eq!((events[0].level.as_str(), field(&events[0], "name")), ("Informação", "srv0"));
    assert_eq!((events[1].level.as_str(), field(&events[1], "caller"), events[1].message.as_str()), ("Erro", "myapp/server.go:42", "listen failed\t{\"port\": 443}"));

    // Exchange message tracking: W3C header with comma-separated values.
    let (format, events) = load(
        "MSGTRK20261012-1.LOG",
        "#Software: Microsoft Exchange Server\n#Version: 15.0.0.0\n#Log-type: Message Tracking Log\n#Fields: date-time,client-ip,event-id,sender-address,message-subject\n\
         2026-10-12T03:00:00.123Z,192.0.2.30,RECEIVE,ana@example.test,\"Relatório, final\"\n",
    );
    assert_eq!(format, "w3c");
    assert_eq!(events[0].timestamp, Some(utc("2026-10-12T03:00:00.123Z")));
    assert_eq!((events[0].code.as_str(), field(&events[0], "message-subject")), ("RECEIVE", "Relatório, final"));
}

#[test]
fn xml_logs_windows_exports_and_java_layouts() {
    let windows = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Events>\n<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Microsoft-Windows-Security-Auditing'/><EventID>4624</EventID><Level>0</Level><TimeCreated SystemTime='2026-10-12T03:00:00.000Z'/><Computer>DC-01</Computer></System><EventData><Data Name='TargetUserName'>alice</Data><Data Name='IpAddress'>192.0.2.8</Data></EventData><RenderingInfo Culture='pt-BR'><Message>Uma conta fez logon com êxito.</Message></RenderingInfo></Event>\n<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Microsoft-Windows-Security-Auditing'/><EventID>4625</EventID><Level>0</Level><TimeCreated SystemTime='2026-10-12T03:00:05.000Z'/><Computer>DC-01</Computer></System><EventData><Data Name='TargetUserName'>bob</Data></EventData></Event>\n</Events>\n";
    let (format, events) = load("Security.xml", windows);
    assert_eq!(format, "xml");
    assert_eq!(events.len(), 2);
    all_dated(&events);
    assert_eq!((events[0].code.as_str(), events[0].message.as_str(), field(&events[0], "TargetUserName")), ("4624", "Uma conta fez logon com êxito.", "alice"));
    assert_eq!(events[1].timestamp, Some(utc("2026-10-12T03:00:05Z")));

    let log4j = "<log4j:event logger=\"com.example.Service\" timestamp=\"1791774000000\" level=\"ERROR\" thread=\"main\"><log4j:message><![CDATA[Falha]]></log4j:message></log4j:event>\n<log4j:event logger=\"com.example.Service\" timestamp=\"1791774001000\" level=\"INFO\" thread=\"main\"><log4j:message>ok</log4j:message></log4j:event>\n";
    let (format, events) = load("app.xml", log4j);
    assert_eq!(format, "xml");
    assert_eq!((events.len(), events[0].level.as_str(), events[0].message.as_str()), (2, "Erro", "Falha"));
}
