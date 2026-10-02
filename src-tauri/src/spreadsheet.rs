//! Spreadsheets as log sources: Excel (xlsx, xlsm, xlsb, xls) and OpenDocument
//! (ods). Every row below the header of every worksheet becomes an event. The
//! workbook is converted once into event snapshots, cached with the other
//! expanded sources, and indexed like any other file.

use crate::model::Event;
use crate::sources::{column_role, date_time_ms, naive_to_ms, normalize_level, parse_timestamp, ColumnRole};
use calamine::{open_workbook_from_rs, Data, DataRef, ExcelDateTime, Reader, SheetType, SheetVisible, Sheets, Xls, Xlsb, Xlsx, Ods};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const EXTENSIONS: [&str; 5] = ["xlsx", "xlsm", "xlsb", "xls", "ods"];
/// Converter version, part of the cached file name: a new version converts again.
const VERSION: u32 = 1;
/// Rows examined before writing: header position and meaning of each column.
const SAMPLE: usize = 200;
/// Titles and notes above the header are skipped within these first rows.
const HEADER_SCAN: usize = 30;

pub fn is_spreadsheet(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Workbooks start with a ZIP (xlsx, xlsm, xlsb, ods) or OLE (xls) signature.
/// Exports named .xls that are really text are read by the text parsers.
fn is_workbook(input: &crate::workspace::CanonicalInput) -> Result<bool, String> {
    let mut head = [0u8; 8];
    let mut file = input.reader()?;
    match file.read_exact(&mut head) {
        Ok(()) => Ok(head.starts_with(b"PK\x03\x04") || head == [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// Feed calamine the already validated regular-file handle. Its path adapter
/// opens the pathname again, which can block on a replaced FIFO before source
/// validation gets a chance to reject that generation. Preserve its exact
/// extension dispatch and fallback order without buffering the whole workbook.
fn open_checked_workbook(input: &crate::workspace::CanonicalInput) -> Result<Sheets<std::io::BufReader<std::fs::File>>, String> {
    let reader = || input.reader().map(std::io::BufReader::new);
    let book = match input.path.extension().and_then(|extension| extension.to_str()) {
        Some("xls" | "xla") => Sheets::Xls(open_workbook_from_rs::<Xls<_>, _>(reader()?).map_err(|e| describe(&e.to_string()))?),
        Some("xlsx" | "xlsm" | "xlam") => Sheets::Xlsx(open_workbook_from_rs::<Xlsx<_>, _>(reader()?).map_err(|e| describe(&e.to_string()))?),
        Some("xlsb") => Sheets::Xlsb(open_workbook_from_rs::<Xlsb<_>, _>(reader()?).map_err(|e| describe(&e.to_string()))?),
        Some("ods") => Sheets::Ods(open_workbook_from_rs::<Ods<_>, _>(reader()?).map_err(|e| describe(&e.to_string()))?),
        _ => {
            if let Ok(book) = open_workbook_from_rs::<Xls<_>, _>(reader()?) { return Ok(Sheets::Xls(book)); }
            if let Ok(book) = open_workbook_from_rs::<Xlsx<_>, _>(reader()?) { return Ok(Sheets::Xlsx(book)); }
            if let Ok(book) = open_workbook_from_rs::<Xlsb<_>, _>(reader()?) { return Ok(Sheets::Xlsb(book)); }
            if let Ok(book) = open_workbook_from_rs::<Ods<_>, _>(reader()?) { return Ok(Sheets::Ods(book)); }
            return Err(describe("Não foi possível identificar o formato."));
        }
    };
    Ok(book)
}

#[derive(Clone, Debug)]
enum Cell {
    Text(String),
    Int(i64),
    Number(f64),
    Bool(bool),
    /// Excel serial date/time (days, fraction for the time of day).
    Date { serial: f64, value: Option<chrono::NaiveDateTime>, duration: bool },
    /// ISO text of OpenDocument dates, times and durations.
    Iso(String),
    Error(String),
}

fn text_cell(text: &str) -> Option<Cell> {
    (!text.trim().is_empty()).then(|| Cell::Text(text.to_string()))
}

fn date_cell(value: &ExcelDateTime) -> Cell {
    Cell::Date { serial: value.as_f64(), value: value.as_datetime(), duration: value.is_duration() }
}

impl Cell {
    fn from_data(data: &Data) -> Option<Cell> {
        Some(match data {
            Data::Empty => return None,
            Data::String(text) => return text_cell(text),
            Data::Int(n) => Cell::Int(*n),
            Data::Float(n) => Cell::Number(*n),
            Data::Bool(b) => Cell::Bool(*b),
            Data::DateTime(value) => date_cell(value),
            Data::DateTimeIso(text) | Data::DurationIso(text) => Cell::Iso(text.clone()),
            Data::Error(error) => Cell::Error(error.to_string()),
        })
    }

    fn from_ref(data: &DataRef<'_>) -> Option<Cell> {
        Some(match data {
            DataRef::Empty => return None,
            DataRef::String(text) => return text_cell(text),
            DataRef::SharedString(text) => return text_cell(text),
            DataRef::Int(n) => Cell::Int(*n),
            DataRef::Float(n) => Cell::Number(*n),
            DataRef::Bool(b) => Cell::Bool(*b),
            DataRef::DateTime(value) => date_cell(value),
            DataRef::DateTimeIso(text) | DataRef::DurationIso(text) => Cell::Iso(text.clone()),
            DataRef::Error(error) => Cell::Error(error.to_string()),
        })
    }

    /// Text shown to people: dates as ISO, integral numbers without decimals.
    fn text(&self) -> String {
        match self {
            Cell::Text(text) | Cell::Iso(text) | Cell::Error(text) => text.clone(),
            Cell::Int(n) => n.to_string(),
            Cell::Number(n) => match number_value(*n) {
                Value::Number(number) => number.to_string(),
                _ => n.to_string(),
            },
            Cell::Bool(b) => b.to_string(),
            Cell::Date { serial, value, duration } => {
                let millis = |days: f64| (days * 86_400_000.0).round() as i64;
                let clock = |ms: i64| {
                    let (h, m, s, rest) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
                    if rest == 0 { format!("{h:02}:{m:02}:{s:02}") } else { format!("{h:02}:{m:02}:{s:02}.{rest:03}") }
                };
                match value {
                    _ if *duration => clock(millis(*serial)),
                    _ if *serial < 1.0 => clock(millis(*serial)),
                    Some(value) if serial.fract() == 0.0 => value.format("%Y-%m-%d").to_string(),
                    Some(value) if value.and_utc().timestamp_subsec_millis() == 0 => value.format("%Y-%m-%d %H:%M:%S").to_string(),
                    Some(value) => value.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
                    None => serial.to_string(),
                }
            }
        }
    }

    fn value(&self) -> Value {
        match self {
            Cell::Int(n) => Value::from(*n),
            Cell::Number(n) => number_value(*n),
            Cell::Bool(b) => Value::from(*b),
            other => Value::from(other.text()),
        }
    }

    fn kind(&self) -> Kind {
        match self {
            Cell::Date { duration: true, .. } => Kind::Other,
            Cell::Date { serial, .. } if *serial < 1.0 => Kind::TimeOnly,
            Cell::Date { serial, .. } if serial.fract() == 0.0 => Kind::DateOnly,
            Cell::Date { .. } => Kind::DateTime,
            Cell::Text(text) | Cell::Iso(text) => text_kind(text),
            _ => Kind::Other,
        }
    }

    /// Event time held by this cell alone.
    fn millis(&self) -> Option<i64> {
        match self {
            Cell::Date { value: Some(value), duration: false, .. } => Some(naive_to_ms(*value)),
            Cell::Date { .. } => None,
            other => parse_timestamp(&other.text()),
        }
    }
}

/// Integral values are written without decimals (identifiers, counts, codes).
fn number_value(n: f64) -> Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9e15 {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map(Value::Number).unwrap_or(Value::Null)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    DateTime,
    DateOnly,
    TimeOnly,
    Other,
}

fn is_clock(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    (2..=3).contains(&parts.len())
        && parts.iter().enumerate().all(|(i, part)| {
            let digits = if i == 2 { part.split(['.', ',']).next().unwrap_or("") } else { part };
            !digits.is_empty() && digits.len() <= 2 && digits.chars().all(|c| c.is_ascii_digit())
        })
}

fn text_kind(text: &str) -> Kind {
    let text = text.trim();
    if is_clock(text) {
        Kind::TimeOnly
    } else if text.parse::<f64>().is_ok() || parse_timestamp(text).is_none() {
        // Numbers in text cells are identifiers far more often than epochs.
        Kind::Other
    } else if text.contains(':') {
        Kind::DateTime
    } else {
        Kind::DateOnly
    }
}

/// Excel column letters: 0 → A, 26 → AA.
fn letter(mut column: u32) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (column % 26) as u8);
        if column < 26 {
            break;
        }
        column = column / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

type Row = (u32, Vec<(u32, Cell)>);

enum Time {
    None,
    Column(u32),
    Parts { date: u32, time: u32 },
}

/// How the rows of one worksheet become events, decided from its first rows.
struct Plan {
    header: Option<u32>,
    names: BTreeMap<u32, String>,
    used: HashSet<String>,
    time: Time,
    level: Option<u32>,
    code: Option<u32>,
    source: Option<u32>,
    message: Option<u32>,
    /// A single text column of log lines, parsed by the text parsers.
    lines: Option<(u32, &'static str)>,
}

/// Code columns of a worksheet: row identifiers and statuses are not event codes.
fn is_code_name(name: &str) -> bool {
    column_role(name) == ColumnRole::Code
        && !matches!(name.trim().to_lowercase().as_str(), "id" | "status" | "situação" | "situacao")
}

fn header_like(text: &str) -> bool {
    let text = text.trim();
    text.chars().count() <= 64
        && text.chars().any(char::is_alphabetic)
        && text.parse::<f64>().is_err()
        && parse_timestamp(text).is_none()
}

impl Plan {
    fn new(rows: &[Row]) -> Plan {
        let scan = &rows[..rows.len().min(HEADER_SCAN)];
        let widest = scan.iter().map(|(_, cells)| cells.len()).max().unwrap_or(0);
        let header_at = (widest >= 2)
            .then(|| {
                scan.iter().position(|(_, cells)| {
                    let texts = cells.iter().filter(|(_, cell)| matches!(cell, Cell::Text(t) if header_like(t))).count();
                    cells.len() * 10 >= widest * 6 && texts * 10 >= cells.len() * 8
                })
            })
            .flatten();
        let data: &[Row] = match header_at {
            Some(i) => &rows[i + 1..],
            None => rows,
        };
        let mut plan = Plan {
            header: header_at.map(|i| rows[i].0),
            names: BTreeMap::new(),
            used: HashSet::new(),
            time: Time::None,
            level: None,
            code: None,
            source: None,
            message: None,
            lines: None,
        };
        if let Some(i) = header_at {
            for (column, cell) in &rows[i].1 {
                let name = cell.text().split_whitespace().collect::<Vec<_>>().join(" ");
                plan.name(*column, Some(name));
            }
        }
        let mut columns: Vec<u32> = data.iter().flat_map(|(_, cells)| cells.iter().map(|(c, _)| *c)).collect();
        columns.sort_unstable();
        columns.dedup();
        for &column in &columns {
            plan.name(column, None);
        }
        // Pasted log lines in a single column keep their own format.
        if header_at.is_none() && columns.len() == 1 {
            let texts: Vec<String> = data.iter().filter_map(|(_, cells)| match &cells[0].1 {
                Cell::Text(text) => Some(text.clone()),
                _ => None,
            }).collect();
            if texts.len() * 10 >= data.len() * 8 && !texts.is_empty() {
                plan.lines = Some((columns[0], crate::sources::detect_format(texts.join("\n").as_bytes())));
                return plan;
            }
        }
        plan.classify(&columns, data);
        plan
    }

    /// Unique field name for a column; `plan.names` keeps the first one given.
    fn name(&mut self, column: u32, header: Option<String>) -> String {
        if let Some(name) = self.names.get(&column) {
            return name.clone();
        }
        let base = header.filter(|h| !h.is_empty()).unwrap_or_else(|| letter(column));
        let base = if base.starts_with("planilha.") { format!("{base} ({})", letter(column)) } else { base };
        let mut name = base.clone();
        let mut n = 2;
        while self.used.contains(&name) {
            name = format!("{base} ({n})");
            n += 1;
        }
        self.used.insert(name.clone());
        self.names.insert(column, name.clone());
        name
    }

    fn classify(&mut self, columns: &[u32], data: &[Row]) {
        let mut filled: BTreeMap<u32, usize> = BTreeMap::new();
        let mut kinds: BTreeMap<(u32, Kind), usize> = BTreeMap::new();
        let mut text_length: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
        for (_, cells) in data {
            for (column, cell) in cells {
                *filled.entry(*column).or_default() += 1;
                *kinds.entry((*column, cell.kind())).or_default() += 1;
                if let Cell::Text(text) = cell {
                    let entry = text_length.entry(*column).or_default();
                    entry.0 += text.chars().count();
                    entry.1 += 1;
                }
            }
        }
        let mostly = |column: u32, kind: Kind| {
            let total = filled.get(&column).copied().unwrap_or(0);
            total > 0 && kinds.get(&(column, kind)).copied().unwrap_or(0) * 10 >= total * 8
        };
        let role = |plan: &Plan, column: u32| column_role(&plan.names[&column]);
        let prefer = |plan: &Plan, candidates: Vec<u32>, roles: &[ColumnRole]| {
            candidates.iter().copied().find(|&c| roles.contains(&role(plan, c))).or(candidates.first().copied())
        };
        let full: Vec<u32> = columns.iter().copied().filter(|&c| mostly(c, Kind::DateTime)).collect();
        let dates: Vec<u32> = columns.iter().copied().filter(|&c| mostly(c, Kind::DateOnly)).collect();
        let times: Vec<u32> = columns.iter().copied().filter(|&c| mostly(c, Kind::TimeOnly)).collect();
        self.time = if let Some(c) = prefer(self, full, &[ColumnRole::Timestamp, ColumnRole::Date, ColumnRole::Time]) {
            Time::Column(c)
        } else {
            match (prefer(self, dates, &[ColumnRole::Date, ColumnRole::Timestamp]), prefer(self, times, &[ColumnRole::Time])) {
                (Some(date), Some(time)) => Time::Parts { date, time },
                (Some(date), None) => Time::Column(date),
                _ => {
                    // Epoch numbers or other formats under a column named as the event time.
                    columns
                        .iter()
                        .copied()
                        .find(|&c| {
                            role(self, c) == ColumnRole::Timestamp && {
                                let values: Vec<&Cell> = data.iter().flat_map(|(_, cells)| cells.iter().filter(|(k, _)| *k == c).map(|(_, v)| v)).collect();
                                !values.is_empty() && values.iter().filter(|v| v.millis().is_some()).count() * 10 >= values.len() * 8
                            }
                        })
                        .map_or(Time::None, Time::Column)
                }
            }
        };
        let time_columns: Vec<u32> = match self.time {
            Time::Column(c) => vec![c],
            Time::Parts { date, time } => vec![date, time],
            Time::None => vec![],
        };
        let first = |plan: &Plan, test: &dyn Fn(&str) -> bool| {
            columns.iter().copied().find(|c| !time_columns.contains(c) && test(&plan.names[c]))
        };
        self.level = first(self, &|name| column_role(name) == ColumnRole::Level);
        self.code = first(self, &is_code_name);
        self.source = first(self, &|name| column_role(name) == ColumnRole::Source);
        self.message = first(self, &|name| column_role(name) == ColumnRole::Message).or_else(|| {
            // The longest text column reads as the message when none is named.
            let taken = [self.level, self.code, self.source];
            text_length
                .iter()
                .filter(|(c, (_, n))| *n > 0 && !time_columns.contains(c) && !taken.contains(&Some(**c)))
                .map(|(c, (total, n))| (*c, *total / *n))
                .filter(|(_, average)| *average >= 30)
                .max_by_key(|(c, average)| (*average, std::cmp::Reverse(*c)))
                .map(|(c, _)| c)
        });
    }

    fn event(&mut self, cells: &[(u32, Cell)], year: i32) -> Event {
        let cell = |column: Option<u32>| column.and_then(|c| cells.iter().find(|(k, _)| *k == c).map(|(_, v)| v));
        if let Some((column, format)) = self.lines {
            let text = cell(Some(column)).map(Cell::text).unwrap_or_default();
            let mut ev = crate::sources::parse_line_at(text.as_bytes(), format, None, &[], year);
            for (c, value) in cells.iter().filter(|(c, _)| *c != column) {
                let name = self.name(*c, None);
                ev.fields.insert(name, value.value());
            }
            return ev;
        }
        let mut ev = Event::empty();
        let mut raw = Vec::with_capacity(cells.len());
        for (column, value) in cells {
            let name = self.name(*column, None);
            raw.push(format!(
                "{}:{}",
                serde_json::to_string(&name).unwrap_or_default(),
                serde_json::to_string(&value.value()).unwrap_or_default()
            ));
            ev.fields.insert(name, value.value());
        }
        ev.raw = format!("{{{}}}", raw.join(","));
        ev.timestamp = match self.time {
            Time::Column(c) => cell(Some(c)).and_then(Cell::millis),
            Time::Parts { date, time } => match (cell(Some(date)), cell(Some(time))) {
                (Some(d), Some(t)) => date_time_ms(Some(&d.text()), Some(&t.text())),
                (Some(d), None) => d.millis(),
                _ => None,
            },
            Time::None => None,
        };
        if let Some(level) = cell(self.level) {
            ev.level = normalize_level(&level.text());
        }
        if let Some(code) = cell(self.code) {
            ev.code = code.text();
        }
        if let Some(source) = cell(self.source) {
            ev.source = source.text();
        }
        ev.message = match cell(self.message) {
            Some(message) => message.text(),
            None => {
                let skip = |c: &u32| matches!(self.time, Time::Column(t) if t == *c) || matches!(self.time, Time::Parts { date, time } if date == *c || time == *c);
                let mut summary = cells
                    .iter()
                    .filter(|(c, _)| !skip(c))
                    .take(6)
                    .map(|(c, v)| format!("{}: {}", self.names.get(c).cloned().unwrap_or_else(|| letter(*c)), v.text().chars().take(80).collect::<String>()))
                    .collect::<Vec<_>>()
                    .join(" · ");
                if summary.chars().count() > 300 {
                    summary = summary.chars().take(300).collect::<String>() + "…";
                }
                summary
            }
        };
        ev
    }
}

/// Rows of one worksheet, in order, with their filled cells.
fn visit_rows<R: Read + std::io::Seek>(
    book: &mut Sheets<R>,
    sheet: &str,
    visit: &mut dyn FnMut(u32, Vec<(u32, Cell)>) -> Result<(), String>,
) -> Result<(), String> {
    let mut pending: Option<Row> = None;
    let mut push = |row: u32, column: u32, cell: Option<Cell>| -> Result<(), String> {
        let Some(cell) = cell else { return Ok(()) };
        match &mut pending {
            Some((current, cells)) if *current == row => cells.push((column, cell)),
            _ => {
                if let Some((number, cells)) = pending.replace((row, vec![(column, cell)])) {
                    visit(number, cells)?;
                }
            }
        }
        Ok(())
    };
    match book {
        // Streamed: large workbooks are never held whole in memory.
        Sheets::Xlsx(book) => {
            let mut reader = book.worksheet_cells_reader(sheet).map_err(|e| e.to_string())?;
            while let Some(cell) = reader.next_cell().map_err(|e| e.to_string())? {
                let (row, column) = cell.get_position();
                push(row, column, Cell::from_ref(cell.get_value()))?;
            }
        }
        Sheets::Xlsb(book) => {
            let mut reader = book.worksheet_cells_reader(sheet).map_err(|e| e.to_string())?;
            while let Some(cell) = reader.next_cell().map_err(|e| e.to_string())? {
                let (row, column) = cell.get_position();
                push(row, column, Cell::from_ref(cell.get_value()))?;
            }
        }
        other => {
            let range = other.worksheet_range(sheet).map_err(|e| e.to_string())?;
            let (first_row, first_column) = range.start().unwrap_or((0, 0));
            for (i, cells) in range.rows().enumerate() {
                for (j, value) in cells.iter().enumerate() {
                    push(first_row + i as u32, first_column + j as u32, Cell::from_data(value))?;
                }
            }
        }
    }
    if let Some((number, cells)) = pending {
        visit(number, cells)?;
    }
    Ok(())
}

fn describe(error: &str) -> String {
    if error.to_lowercase().contains("password") {
        "A planilha está protegida por senha. Salve uma cópia sem senha para abri-la.".into()
    } else {
        format!("Não foi possível ler a planilha: {error}")
    }
}

/// Writes one event per row of every worksheet; returns how many were written.
fn convert(input: &crate::workspace::CanonicalInput, workbook_id: &str, year: i32, out: &mut dyn Write, progress: &dyn Fn(usize)) -> Result<usize, String> {
    let mut book = open_checked_workbook(input)?;
    let sheets: Vec<_> = book.sheets_metadata().to_vec();
    let mut written = 0usize;
    let mut failures = Vec::new();
    for (sheet_index, sheet) in sheets.iter().enumerate() {
        if sheet.typ != SheetType::WorkSheet {
            continue;
        }
        let hidden = sheet.visible != SheetVisible::Visible;
        let mut buffer: Vec<Row> = Vec::new();
        let mut plan: Option<Plan> = None;
        let mut write = |plan: &mut Plan, row: u32, cells: &[(u32, Cell)], written: &mut usize| -> Result<(), String> {
            if plan.header.is_some_and(|header| row <= header) {
                return Ok(());
            }
            let mut ev = plan.event(cells, year);
            ev.event_ref = format!("planilha:{}:{sheet_index}:{}", &workbook_id[..16], row + 1);
            ev.fields.insert("planilha.aba".into(), Value::from(sheet.name.as_str()));
            ev.fields.insert("planilha.linha".into(), Value::from(row + 1));
            if hidden {
                ev.fields.insert("planilha.oculta".into(), Value::from(true));
            }
            serde_json::to_writer(&mut *out, &ev).map_err(|e| e.to_string())?;
            out.write_all(b"\n").map_err(|e| e.to_string())?;
            *written += 1;
            if *written % 2048 == 0 {
                crate::operations::check()?;
                progress(*written);
            }
            Ok(())
        };
        let result = visit_rows(&mut book, &sheet.name, &mut |row, cells| {
            match plan.as_mut() {
                Some(plan) => write(plan, row, &cells, &mut written),
                None => {
                    buffer.push((row, cells));
                    if buffer.len() >= SAMPLE {
                        let mut ready = Plan::new(&buffer);
                        for (row, cells) in buffer.drain(..) {
                            write(&mut ready, row, &cells, &mut written)?;
                        }
                        plan = Some(ready);
                    }
                    Ok(())
                }
            }
        });
        if let Err(error) = result {
            // A cancelled load stops; an unreadable sheet (charts, corruption) is skipped.
            if crate::operations::cancelled() {
                return Err(error);
            }
            failures.push(format!("{}: {error}", sheet.name));
            continue;
        }
        if plan.is_none() && !buffer.is_empty() {
            let mut ready = Plan::new(&buffer);
            for (row, cells) in buffer.drain(..) {
                write(&mut ready, row, &cells, &mut written)?;
            }
        }
    }
    if written == 0 {
        return Err(match failures.first() {
            Some(failure) => describe(failure),
            None => "A planilha não tem linhas com dados.".into(),
        });
    }
    Ok(written)
}

/// Converts the workbook once and returns the event snapshots to index, or
/// `None` when the file is not a workbook and must be read as text.
pub fn expand(path: &Path, progress: &dyn Fn(usize)) -> Result<Option<PathBuf>, String> {
    let input = crate::workspace::CanonicalInput::open(path)?;
    if !is_workbook(&input)? { return Ok(None); }
    let legacy = input.legacy_source().map(|(path, id)| {
        let stem: String = path.file_stem().unwrap_or_default().to_string_lossy().chars()
            .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' }).take(60).collect();
        crate::config_dir().join("expanded").join(format!("{}-planilha-v{VERSION}-{stem}.jsonl", &id[..16]))
    });
    // Explicit planilha event references retain the original/validated member
    // identity; the physical generation only selects the disposable cache.
    let calendar = crate::sources::ParserCalendar::current()?;
    let converter = format!("workbook-v{VERSION}-{}-{}", calendar.year, calendar.timezone);
    calendar.validate_timezone()?;
    let output = crate::workspace::canonical_file(input, &converter, legacy, |input, out| {
        calendar.validate_timezone()?;
        convert(input, &input.logical_identity, calendar.year, out, progress)?;
        calendar.validate_timezone()
    })?;
    calendar.validate_timezone()?;
    Ok(Some(output))
}


#[cfg(test)]
mod calendar_tests {
    use super::*;

    #[test]
    fn single_column_logs_use_the_workbooks_pinned_parser_year() {
        let rows = vec![(0, vec![(0, Cell::Text("Jan 31 08:03:10 srv sshd[22]: Accepted password for root from 192.0.2.1 port 22 ssh2".into()))])];
        let mut plan = Plan::new(&rows);
        assert!(plan.lines.is_some());
        let event = plan.event(&rows[0].1, 2021);
        let timestamp = event.timestamp.expect("syslog timestamp");
        use chrono::TimeZone;
        assert_eq!(chrono::Datelike::year(&chrono::Local.timestamp_millis_opt(timestamp).single().unwrap()), 2021);
    }
}

#[cfg(test)]
mod checked_reader_tests {
    use super::*;

    fn workbook(path: &Path) {
        let mut book = rust_xlsxwriter::Workbook::new();
        let sheet = book.add_worksheet();
        sheet.write_string(0, 0, "message").unwrap();
        sheet.write_string(1, 0, "preserved value").unwrap();
        book.save(path).unwrap();
    }

    #[test]
    fn regular_workbooks_keep_extension_fallback_and_text_xls_behavior() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["events.xlsx", "events.XLSX", "events.unknown"] {
            let path = directory.path().join(name);
            workbook(&path);
            let input = crate::workspace::CanonicalInput::open(&path).unwrap();
            assert!(is_workbook(&input).unwrap());
            let mut book = open_checked_workbook(&input).unwrap();
            let sheet = book.sheet_names()[0].clone();
            assert_eq!(book.worksheet_range(&sheet).unwrap().get_value((1, 0)), Some(&Data::String("preserved value".into())));
        }
        let text = directory.path().join("export.xls");
        std::fs::write(&text, b"message\ntext export\n").unwrap();
        assert!(expand(&text, &|_| {}).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn workbook_special_file_child() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let Some(root) = std::env::var_os("LOGINSIGHT_WORKBOOK_SPECIAL_ROOT") else { return };
        let root = PathBuf::from(root);
        let fifo = |path: &Path| {
            let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        };
        let path = root.join("blocked.xlsx");
        fifo(&path);
        assert!(expand(&path, &|_| {}).unwrap_err().contains("regular"));
        std::fs::remove_file(&path).unwrap();
        workbook(&path);
        let input = crate::workspace::CanonicalInput::open(&path).unwrap();
        let retained = root.join("retained.xlsx");
        std::fs::rename(&path, &retained).unwrap();
        fifo(&path);
        assert!(is_workbook(&input).unwrap_err().contains("regular"));
        assert!(open_checked_workbook(&input).is_err());
        let link = root.join("regular-link.xlsx");
        symlink(&retained, &link).unwrap();
        let linked = crate::workspace::CanonicalInput::open(&link).unwrap();
        assert!(is_workbook(&linked).unwrap());
        assert!(open_checked_workbook(&linked).is_ok());
        let token = crate::operations::token(Some("cancel-workbook-probe".into())).unwrap();
        assert!(crate::operations::run_with_token(token, || {
            crate::operations::cancel_id("cancel-workbook-probe");
            expand(&path, &|_| {})
        }).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn initial_and_replaced_workbook_fifos_never_wait_for_a_writer() {
        let directory = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "spreadsheet::checked_reader_tests::workbook_special_file_child", "--test-threads=1"])
            .env("LOGINSIGHT_WORKBOOK_SPECIAL_ROOT", directory.path()).spawn().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() { assert!(status.success()); break; }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill(); let _ = child.wait();
                panic!("workbook admission waited on a special file");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
