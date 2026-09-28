//! Spreadsheets, CSV/TSV with any separator and text in UTF-16 or Windows-1252,
//! loaded through the same path as the app.
use loginsight_lib::testkit::{self, Event};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Caches and expanded files go to a temporary data folder shared by the tests.
fn workdir() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("data"));
        dir
    })
    .path()
}

fn file(name: &str, bytes: &[u8]) -> PathBuf {
    let path = workdir().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn load(path: &Path) -> (String, Vec<Event>) {
    testkit::load(path.to_str().unwrap()).unwrap()
}

fn field<'a>(event: &'a Event, name: &str) -> Option<&'a Value> {
    event.fields.get(name)
}

/// Local wall-clock time, as the app reads times without a zone.
fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    use chrono::TimeZone;
    chrono::Local.with_ymd_and_hms(y, mo, d, h, mi, s).single().unwrap().timestamp_millis()
}

#[test]
fn semicolon_csv_from_excel_in_portuguese() {
    let path = file(
        "acessos.csv",
        "\u{feff}Data;Hora;Usuário;Nível;Mensagem;Valor\n28/09/2026;10:15:30;ana;erro;Falha no login;1,5\n28/09/2026;10:16;bruno;info;Login ok;2\n"
            .as_bytes(),
    );
    let (format, events) = load(&path);
    assert_eq!(format, "csv-semicolon");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, Some(local(2026, 9, 28, 10, 15, 30)));
    assert_eq!(events[1].timestamp, Some(local(2026, 9, 28, 10, 16, 0)));
    assert_eq!(events[0].level, "Erro");
    assert_eq!(events[0].message, "Falha no login");
    assert_eq!(field(&events[0], "Usuário"), Some(&Value::from("ana")));
    assert_eq!(field(&events[0], "Valor"), Some(&Value::from("1,5")));
    assert!(field(&events[0], "Data").is_none(), "date and time became the event time");
}

#[test]
fn tab_and_pipe_separated_values() {
    let (format, events) = load(&file("eventos.tsv", b"timestamp\tuser\taction\n2026-09-28 10:00:00\tana\tlogin\n2026-09-28 10:01:00\tbruno\tlogout\n"));
    assert_eq!(format, "tsv");
    assert_eq!(field(&events[1], "action"), Some(&Value::from("logout")));
    let (format, events) = load(&file("eventos.psv", b"time|host|status|path\n2026-09-28T10:00:00Z|srv-1|500|/api\n2026-09-28T10:00:01Z|srv-2|200|/\n"));
    assert_eq!(format, "csv-pipe");
    assert_eq!(events[0].source, "srv-1");
    assert_eq!(events[0].code, "500");
}

#[test]
fn windows_event_viewer_export_keeps_the_unnamed_message() {
    let csv = "Nível,Data e Hora,Fonte,Identificação do Evento,Categoria da Tarefa\nInformações,28/09/2026 10:15:30,Microsoft-Windows-Security-Auditing,4624,Logon,\"Uma conta fez logon com êxito.\"\n";
    let (format, events) = load(&file("seguranca.csv", csv.as_bytes()));
    assert_eq!(format, "csv");
    assert_eq!(events[0].code, "4624");
    assert_eq!(events[0].source, "Microsoft-Windows-Security-Auditing");
    assert_eq!(events[0].message, "Uma conta fez logon com êxito.");
    assert_eq!(events[0].timestamp, Some(local(2026, 9, 28, 10, 15, 30)));
}

#[test]
fn iis_date_and_time_columns_keep_the_time_of_day() {
    let log = "#Software: Microsoft Internet Information Services 10.0\n#Fields: date time s-ip cs-method cs-uri-stem sc-status\n2026-09-28 10:15:30 10.0.0.9 GET /api/x 500\n";
    let (format, events) = load(&file("u_ex260928.log", log.as_bytes()));
    assert_eq!(format, "w3c");
    assert_eq!(events[0].timestamp, Some(local(2026, 9, 28, 10, 15, 30)));
    assert_eq!(events[0].code, "500");
}

#[test]
fn other_formats_are_not_mistaken_for_tables() {
    assert_eq!(testkit::detect_format(b"{\"a\":1,\"b\":2,\"c\":3}\n{\"a\":4,\"b\":5,\"c\":6}\n"), "jsonl");
    assert_eq!(testkit::detect_format(b"Sep 28 10:00:00 host app[1]: user=a, action=b, result=c\nSep 28 10:00:01 host app[1]: user=d, action=e, result=f\n"), "syslog3164");
    assert_eq!(testkit::detect_format(b"timestamp,level,message\n2024-01-31,info,ok\n"), "csv");
}

#[test]
fn utf16_and_windows_1252_text_is_read_correctly() {
    let text = "2026-09-28 10:15:30 ERROR Falha de autenticação para usuário joão\n2026-09-28 10:15:31 INFO Sessão iniciada\n";
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        utf16.extend(unit.to_le_bytes());
    }
    assert_eq!(testkit::encoding_of(&utf16), Some("UTF-16LE"));
    let (_, events) = load(&file("powershell.log", &utf16));
    assert_eq!(events.len(), 2);
    assert!(events[0].raw.contains("autenticação para usuário joão"), "{}", events[0].raw);
    let latin: Vec<u8> = text.chars().map(|c| c as u32 as u8).collect();
    assert_eq!(testkit::encoding_of(&latin), Some("windows-1252"));
    let (_, events) = load(&file("ansi.log", &latin));
    assert!(events[1].raw.contains("Sessão iniciada"), "{}", events[1].raw);
    assert_eq!(testkit::encoding_of(text.as_bytes()), None, "UTF-8 stays as it is");
}

#[test]
fn excel_workbook_rows_become_events() {
    use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};
    let path = workdir().join("relatorio.xlsx");
    let mut book = Workbook::new();
    let date = Format::new().set_num_format("dd/mm/yyyy");
    let time = Format::new().set_num_format("hh:mm:ss");
    let stamp = Format::new().set_num_format("dd/mm/yyyy hh:mm");
    {
        let sheet = book.add_worksheet().set_name("Acessos").unwrap();
        sheet.write_string(0, 0, "Relatório de acessos — setembro").unwrap();
        for (column, name) in ["Data", "Hora", "Usuário", "IP", "Descrição", "Bytes", "Bloqueado"].iter().enumerate() {
            sheet.write_string(2, column as u16, *name).unwrap();
        }
        let rows = [
            (28, 10, 15, 30, "ana", "10.0.0.5", "Tentativa de acesso ao painel administrativo negada pelo firewall", 1532.0, true),
            (28, 11, 0, 0, "bruno", "10.0.0.9", "Download do relatório financeiro concluído com sucesso pelo usuário", 20480.5, false),
        ];
        for (i, (day, h, m, s, user, ip, text, bytes, blocked)) in rows.iter().enumerate() {
            let row = 3 + i as u32;
            sheet.write_datetime_with_format(row, 0, &ExcelDateTime::from_ymd(2026, 9, *day).unwrap(), &date).unwrap();
            sheet.write_datetime_with_format(row, 1, &ExcelDateTime::from_hms(*h, *m, *s as f64).unwrap(), &time).unwrap();
            sheet.write_string(row, 2, *user).unwrap();
            sheet.write_string(row, 3, *ip).unwrap();
            sheet.write_string(row, 4, *text).unwrap();
            sheet.write_number(row, 5, *bytes).unwrap();
            sheet.write_boolean(row, 6, *blocked).unwrap();
        }
    }
    {
        let sheet = book.add_worksheet().set_name("Auditoria").unwrap();
        sheet.set_hidden(true);
        for (column, name) in ["Timestamp", "Level", "Message", "ID"].iter().enumerate() {
            sheet.write_string(0, column as u16, *name).unwrap();
        }
        sheet.write_datetime_with_format(1, 0, &ExcelDateTime::from_ymd(2026, 9, 29).unwrap().and_hms(8, 30, 0).unwrap(), &stamp).unwrap();
        sheet.write_string(1, 1, "warning").unwrap();
        sheet.write_string(1, 2, "Senha alterada").unwrap();
        sheet.write_number(1, 3, 7.0).unwrap();
    }
    {
        let sheet = book.add_worksheet().set_name("Syslog").unwrap();
        sheet.write_string(0, 0, "Sep 28 10:00:00 srv-web sshd[812]: Failed password for root from 45.90.12.3 port 22 ssh2").unwrap();
        sheet.write_string(1, 0, "Sep 28 10:00:05 srv-web sshd[812]: Accepted password for ana from 10.0.0.5 port 22 ssh2").unwrap();
    }
    book.save(&path).unwrap();

    let (format, events) = load(&path);
    assert_eq!(format, "snapshot");
    assert_eq!(events.len(), 5, "{:#?}", events.iter().map(|e| &e.message).collect::<Vec<_>>());
    let first = &events[0];
    assert_eq!(first.timestamp, Some(local(2026, 9, 28, 10, 15, 30)));
    assert_eq!(first.message, "Tentativa de acesso ao painel administrativo negada pelo firewall");
    assert_eq!(field(first, "Usuário"), Some(&Value::from("ana")));
    assert_eq!(field(first, "Bytes"), Some(&Value::from(1532)));
    assert_eq!(field(first, "Bloqueado"), Some(&Value::from(true)));
    assert_eq!(field(first, "Data"), Some(&Value::from("2026-09-28")));
    assert_eq!(field(first, "Hora"), Some(&Value::from("10:15:30")));
    assert_eq!(field(first, "planilha.aba"), Some(&Value::from("Acessos")));
    assert_eq!(field(first, "planilha.linha"), Some(&Value::from(4)));
    assert_eq!(field(&events[1], "Bytes"), Some(&Value::from(20480.5)));
    let audit = &events[2];
    assert_eq!(audit.timestamp, Some(local(2026, 9, 29, 8, 30, 0)));
    assert_eq!(audit.level, "Aviso");
    assert_eq!(audit.message, "Senha alterada");
    assert_eq!(audit.code, "", "row identifiers are not event codes");
    assert_eq!(field(audit, "planilha.oculta"), Some(&Value::from(true)));
    let syslog = &events[3];
    assert!(syslog.message.contains("Failed password for root"), "{}", syslog.message);
    assert_eq!(syslog.source, "srv-web");
    assert_eq!(field(syslog, "planilha.aba"), Some(&Value::from("Syslog")));
    assert!(events.iter().all(|e| e.event_ref.starts_with("planilha:")));
}

#[test]
fn opendocument_spreadsheet() {
    let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2"><office:body><office:spreadsheet><table:table table:name="Planilha1">
<table:table-row><table:table-cell office:value-type="string"><text:p>Data/Hora</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>Evento</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>Usuário</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell office:value-type="date" office:date-value="2026-09-28T10:15:30"><text:p>28/09/2026 10:15:30</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>Login com falha</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>ana</text:p></table:table-cell></table:table-row>
</table:table></office:spreadsheet></office:body></office:document-content>"#;
    let manifest = r#"<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#;
    let path = workdir().join("planilha.ods");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.oasis.opendocument.spreadsheet").unwrap();
    zip.start_file("META-INF/manifest.xml", stored).unwrap();
    zip.write_all(manifest.as_bytes()).unwrap();
    zip.start_file("content.xml", stored).unwrap();
    zip.write_all(content.as_bytes()).unwrap();
    zip.finish().unwrap();

    let (_, events) = load(&path);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].timestamp, Some(local(2026, 9, 28, 10, 15, 30)));
    assert_eq!(events[0].message, "Login com falha");
    assert_eq!(field(&events[0], "Usuário"), Some(&Value::from("ana")));
}

#[test]
fn xls_exports_that_are_text_are_read_as_text() {
    let (format, events) = load(&file("exportacao.xls", b"Data\tUsuario\tAcao\n28/09/2026 10:00:00\tana\tlogin\n28/09/2026 10:05:00\tbruno\tlogout\n"));
    assert_eq!(format, "tsv");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, Some(local(2026, 9, 28, 10, 0, 0)));
}
