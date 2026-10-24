//! Typed conversion of extracted values: ordered normalization steps, then an optional
//! type. Recognising a pattern is not converting it: nothing here runs unless the map's
//! output asks for it, and the original value is always kept elsewhere (provenance).
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ValueType {
    Text,
    Integer,
    Decimal,
    Boolean,
    Datetime,
    Ip,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Step {
    Trim,
    Lowercase,
    Uppercase,
    Basename,
    /// `0x2a`, `2A` or `-0x10` → decimal text.
    HexToInt,
    StripPrefix { value: String },
    StripSuffix { value: String },
    RemoveChars { chars: String },
    Replace { from: String, to: String },
}
/// How text becomes an instant. A format without an offset needs an explicit time zone:
/// `"utc"` or a fixed offset such as `"+03:00"`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DatetimeSpec {
    /// `iso8601`, `epoch_s`, `epoch_ms` or a chrono/strftime pattern (`%d/%b/%Y:%H:%M:%S %z`).
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub timezone: String,
    /// For formats without a year (syslog), the year to assume.
    #[serde(default)]
    pub default_year: Option<i32>,
}
pub(crate) enum Converted {
    Empty,
    Invalid(String),
    Value(Value),
}

pub(crate) fn validate_spec(ty: Option<ValueType>, steps: &[Step], datetime: Option<&DatetimeSpec>) -> Result<(), String> {
    if steps.len() > 12 {
        return Err("Use até 12 conversões por saída.".into());
    }
    for step in steps {
        match step {
            Step::StripPrefix { value } | Step::StripSuffix { value } if value.is_empty() || value.len() > 256 => return Err("Prefixo ou sufixo vazio ou excessivo.".into()),
            Step::RemoveChars { chars } if chars.is_empty() || chars.len() > 64 => return Err("Informe de 1 a 64 caracteres a remover.".into()),
            Step::Replace { from, to } if from.is_empty() || from.len() > 256 || to.len() > 256 => return Err("Texto a substituir vazio ou excessivo.".into()),
            _ => {}
        }
    }
    match (ty, datetime) {
        (Some(ValueType::Datetime), None) => Err("Informe o formato e o fuso horário da data.".into()),
        (Some(ValueType::Datetime), Some(spec)) => {
            let format = spec.format.as_str();
            if format.is_empty() || format.len() > 128 {
                return Err("Informe o formato da data (iso8601, epoch_s, epoch_ms ou um padrão como %d/%m/%Y %H:%M:%S).".into());
            }
            let has_offset = format == "iso8601" || format.contains("%z") || format.contains("%:z") || format.contains("%#z");
            let numeric_epoch = matches!(format, "epoch_s" | "epoch_ms");
            if !has_offset && !numeric_epoch && parse_zone(&spec.timezone).is_none() {
                return Err("Este formato de data não informa o fuso horário. Escolha “utc” ou um deslocamento fixo como +03:00.".into());
            }
            Ok(())
        }
        (_, Some(_)) => Err("A configuração de data só se aplica ao tipo data/hora.".into()),
        _ => Ok(()),
    }
}
/// `utc`, `+03:00`, `-0300`, `+3` → offset in seconds.
fn parse_zone(zone: &str) -> Option<i32> {
    let zone = zone.trim();
    if zone.eq_ignore_ascii_case("utc") || zone == "Z" {
        return Some(0);
    }
    let sign = match zone.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits: String = zone[1..].chars().filter(|c| *c != ':').collect();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) || digits.len() > 4 {
        return None;
    }
    let (h, m) = if digits.len() <= 2 { (digits.parse::<i32>().ok()?, 0) } else { (digits[..digits.len() - 2].parse::<i32>().ok()?, digits[digits.len() - 2..].parse::<i32>().ok()?) };
    (h <= 14 && m < 60).then_some(sign * (h * 3600 + m * 60))
}
fn apply_step(text: String, step: &Step) -> Result<String, String> {
    Ok(match step {
        Step::Trim => text.trim().to_owned(),
        Step::Lowercase => text.to_lowercase(),
        Step::Uppercase => text.to_uppercase(),
        Step::Basename => text.rsplit(['\\', '/']).next().unwrap_or_default().to_owned(),
        Step::HexToInt => {
            let t = text.trim();
            let (negative, digits) = match t.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, t.strip_prefix('+').unwrap_or(t)),
            };
            let digits = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")).unwrap_or(digits);
            let n = u64::from_str_radix(digits, 16).map_err(|_| format!("“{t}” não é um número hexadecimal."))?;
            format!("{}{n}", if negative { "-" } else { "" })
        }
        Step::StripPrefix { value } => text.strip_prefix(value.as_str()).unwrap_or(&text).to_owned(),
        Step::StripSuffix { value } => text.strip_suffix(value.as_str()).unwrap_or(&text).to_owned(),
        Step::RemoveChars { chars } => text.chars().filter(|c| !chars.contains(*c)).collect(),
        Step::Replace { from, to } => text.replace(from.as_str(), to),
    })
}
fn iso(ms: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(ms).map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}
fn parse_datetime(text: &str, spec: &DatetimeSpec) -> Result<String, String> {
    let text = text.trim();
    let bad = || format!("“{text}” não corresponde ao formato de data {}.", if spec.format.is_empty() { "informado" } else { &spec.format });
    let ms = match spec.format.as_str() {
        "epoch_s" => {
            let n: f64 = text.parse().map_err(|_| bad())?;
            (n * 1000.0).round() as i64
        }
        "epoch_ms" => text.parse::<f64>().map_err(|_| bad())?.round() as i64,
        "iso8601" => DateTime::parse_from_rfc3339(text)
            .map(|d| d.timestamp_millis())
            .or_else(|_| {
                let offset = parse_zone(&spec.timezone).ok_or_else(bad)?;
                NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
                    .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f"))
                    .map_err(|_| bad())
                    .and_then(|n| chrono::FixedOffset::east_opt(offset).and_then(|z| z.from_local_datetime(&n).single()).map(|d| d.timestamp_millis()).ok_or_else(bad))
            })?,
        format => {
            let owned;
            let (format, input): (&str, &str) = match spec.default_year {
                Some(year) => {
                    owned = (format!("%Y {format}"), format!("{year} {text}"));
                    (&owned.0, &owned.1)
                }
                None => (format, text),
            };
            if format.contains("%z") || format.contains("%:z") || format.contains("%#z") {
                DateTime::parse_from_str(input, format).map_err(|_| bad())?.timestamp_millis()
            } else {
                let naive = NaiveDateTime::parse_from_str(input, format).map_err(|_| bad())?;
                let offset = parse_zone(&spec.timezone).ok_or_else(bad)?;
                chrono::FixedOffset::east_opt(offset).and_then(|z| z.from_local_datetime(&naive).single()).map(|d| d.timestamp_millis()).ok_or_else(bad)?
            }
        }
    };
    iso(ms).ok_or_else(bad)
}
fn integer(text: &str) -> Result<Value, String> {
    let t = text.trim();
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("“{t}” não é um número inteiro."));
    }
    let n: i128 = t.parse().map_err(|_| format!("“{t}” é grande demais."))?;
    if n > u64::MAX as i128 || n < i64::MIN as i128 {
        return Err(format!("“{t}” está fora do intervalo de inteiros suportado."));
    }
    // Numbers beyond 2^53 cross the interface as text so no digit is lost.
    Ok(if n.abs() <= (1i128 << 53) { json!(n as i64) } else { json!(n.to_string()) })
}
fn decimal(text: &str) -> Result<Value, String> {
    let t = text.trim();
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    let valid = (!whole.is_empty() || !fraction.is_empty()) && whole.bytes().all(|b| b.is_ascii_digit()) && fraction.bytes().all(|b| b.is_ascii_digit());
    if !valid {
        return Err(format!("“{t}” não é um número decimal."));
    }
    let normalized = format!("{}{}{}{}", if t.starts_with('-') { "-" } else { "" }, if whole.is_empty() { "0" } else { whole }, if fraction.is_empty() { "" } else { "." }, fraction);
    let f: f64 = normalized.parse().map_err(|_| format!("“{t}” não é um número decimal."))?;
    // A JSON number only when it reproduces the text exactly; otherwise the text itself.
    Ok(if f.is_finite() && format!("{f}") == normalized { json!(f) } else { json!(normalized) })
}
fn boolean(text: &str) -> Result<Value, String> {
    match text.trim().to_lowercase().as_str() {
        "true" | "t" | "yes" | "y" | "sim" | "s" | "1" | "on" | "verdadeiro" => Ok(json!(true)),
        "false" | "f" | "no" | "n" | "não" | "nao" | "0" | "off" | "falso" => Ok(json!(false)),
        other => Err(format!("“{other}” não é um valor booleano reconhecido.")),
    }
}
/// Steps first, then the type. `None` type keeps the (stepped) text.
pub(crate) fn convert(input: &str, steps: &[Step], ty: Option<ValueType>, datetime: Option<&DatetimeSpec>) -> Converted {
    if input.is_empty() {
        return Converted::Empty;
    }
    let mut text = input.to_owned();
    for step in steps {
        text = match apply_step(text, step) {
            Ok(t) => t,
            Err(e) => return Converted::Invalid(e),
        };
    }
    if text.is_empty() {
        return Converted::Empty;
    }
    let result = match ty.unwrap_or(ValueType::Text) {
        ValueType::Text => Ok(json!(text)),
        ValueType::Integer => integer(&text),
        ValueType::Decimal => decimal(&text),
        ValueType::Boolean => boolean(&text),
        ValueType::Datetime => parse_datetime(&text, datetime.unwrap_or(&DatetimeSpec::default())).map(Value::String),
        ValueType::Ip => text.trim().parse::<std::net::IpAddr>().map(|ip| json!(ip.to_string())).map_err(|_| format!("“{}” não é um endereço IP.", text.trim())),
    };
    match result {
        Ok(value) => Converted::Value(value),
        Err(reason) => Converted::Invalid(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(input: &str, steps: &[Step], ty: ValueType, dt: Option<DatetimeSpec>) -> Value {
        match convert(input, steps, Some(ty), dt.as_ref()) {
            Converted::Value(v) => v,
            Converted::Empty => json!("<empty>"),
            Converted::Invalid(e) => json!(format!("<invalid:{e}>")),
        }
    }
    #[test]
    fn recognised_text_is_only_converted_when_asked() {
        assert_eq!(value("00123", &[], ValueType::Text, None), json!("00123"));
        assert_eq!(value("00123", &[], ValueType::Integer, None), json!(123));
        assert_eq!(value("0x2a", &[Step::HexToInt], ValueType::Integer, None), json!(42));
        assert!(value("0x2a", &[], ValueType::Integer, None).as_str().unwrap().starts_with("<invalid"));
        assert_eq!(value("  ABC  ", &[Step::Trim, Step::Lowercase], ValueType::Text, None), json!("abc"));
        assert_eq!(value("C:\\x\\cmd.exe", &[Step::Basename], ValueType::Text, None), json!("cmd.exe"));
    }
    #[test]
    fn missing_empty_invalid_zero_and_false_are_different_states() {
        assert!(matches!(convert("", &[], Some(ValueType::Integer), None), Converted::Empty));
        assert!(matches!(convert("x", &[], Some(ValueType::Integer), None), Converted::Invalid(_)));
        assert!(matches!(convert("0", &[], Some(ValueType::Integer), None), Converted::Value(v) if v == json!(0)));
        assert!(matches!(convert("false", &[], Some(ValueType::Boolean), None), Converted::Value(v) if v == json!(false)));
        assert!(matches!(convert("  ", &[Step::Trim], Some(ValueType::Text), None), Converted::Empty));
    }
    #[test]
    fn large_numbers_keep_every_digit_across_the_interface() {
        assert_eq!(value("9007199254740993", &[], ValueType::Integer, None), json!("9007199254740993"));
        assert_eq!(value("18446744073709551615", &[], ValueType::Integer, None), json!("18446744073709551615"));
        assert!(value("18446744073709551616", &[], ValueType::Integer, None).as_str().unwrap().starts_with("<invalid"));
        assert_eq!(value("12.50", &[], ValueType::Decimal, None), json!("12.50"));
        assert_eq!(value("12.5", &[], ValueType::Decimal, None), json!(12.5));
        assert_eq!(value(".5", &[], ValueType::Decimal, None), json!(0.5));
        assert_eq!(value("1,5", &[Step::Replace { from: ",".into(), to: ".".into() }], ValueType::Decimal, None), json!(1.5));
    }
    #[test]
    fn dates_need_a_format_and_an_explicit_zone() {
        assert!(validate_spec(Some(ValueType::Datetime), &[], Some(&DatetimeSpec { format: "%d/%m/%Y %H:%M:%S".into(), ..Default::default() })).is_err());
        let spec = DatetimeSpec { format: "%d/%m/%Y %H:%M:%S".into(), timezone: "-03:00".into(), default_year: None };
        validate_spec(Some(ValueType::Datetime), &[], Some(&spec)).unwrap();
        assert_eq!(value("23/10/2026 17:05:09", &[], ValueType::Datetime, Some(spec)), json!("2026-10-23T20:05:09.000Z"));
        let http = DatetimeSpec { format: "%d/%b/%Y:%H:%M:%S %z".into(), ..Default::default() };
        validate_spec(Some(ValueType::Datetime), &[], Some(&http)).unwrap();
        assert_eq!(value("23/Oct/2026:17:05:09 +0200", &[], ValueType::Datetime, Some(http)), json!("2026-10-23T15:05:09.000Z"));
        let syslog = DatetimeSpec { format: "%b %e %H:%M:%S".into(), timezone: "utc".into(), default_year: Some(2026) };
        assert_eq!(value("Oct 23 17:05:09", &[], ValueType::Datetime, Some(syslog)), json!("2026-10-23T17:05:09.000Z"));
        let iso = DatetimeSpec { format: "iso8601".into(), timezone: "utc".into(), default_year: None };
        assert_eq!(value("2026-10-23T17:05:09.5+03:00", &[], ValueType::Datetime, Some(iso.clone())), json!("2026-10-23T14:05:09.500Z"));
        assert_eq!(value("2026-10-23 17:05:09", &[], ValueType::Datetime, Some(iso)), json!("2026-10-23T17:05:09.000Z"));
        let epoch = DatetimeSpec { format: "epoch_s".into(), timezone: String::new(), default_year: None };
        assert_eq!(value("1700000000", &[], ValueType::Datetime, Some(epoch)), json!("2023-11-14T22:13:20.000Z"));
        assert!(value("ontem", &[], ValueType::Datetime, Some(DatetimeSpec { format: "iso8601".into(), timezone: "utc".into(), default_year: None })).as_str().unwrap().starts_with("<invalid"));
    }
    #[test]
    fn ip_and_boolean_are_validated_and_canonical() {
        assert_eq!(value("2001:0DB8:0:0:0:0:0:1", &[], ValueType::Ip, None), json!("2001:db8::1"));
        assert!(value("10.0.0.256", &[], ValueType::Ip, None).as_str().unwrap().starts_with("<invalid"));
        assert_eq!(value("Sim", &[], ValueType::Boolean, None), json!(true));
    }
}
