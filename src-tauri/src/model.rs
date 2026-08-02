use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::HashMap;

/// Colunas fixas que todo evento possui (além dos campos dinâmicos).
pub const STANDARD_COLUMNS: &[&str] = &[
    "timestamp",
    "source",
    "level",
    "code",
    "name",
    "description",
    "message",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: usize,
    /// Epoch em milissegundos (UTC). `None` quando a linha não tem data reconhecível.
    pub timestamp: Option<i64>,
    pub source: String,
    pub level: String,
    pub code: String,
    pub name: String,
    pub description: String,
    pub message: String,
    pub raw: String,
    #[serde(default)]
    pub fields: Map<String, Value>,
}

impl Event {
    pub fn empty() -> Self {
        Event {
            id: 0,
            timestamp: None,
            source: String::new(),
            level: "Informação".into(),
            code: String::new(),
            name: String::new(),
            description: String::new(),
            message: String::new(),
            raw: String::new(),
            fields: Map::new(),
        }
    }

    /// Enriquece o evento com nome/descrição do catálogo do usuário,
    /// caindo para o catálogo extraído do sistema quando não houver entrada.
    pub fn enrich(&mut self, codes: &CodesConfig, fallback: &CodesConfig) {
        if self.code.is_empty() {
            return;
        }
        if let Some(info) = codes
            .lookup(&self.source, &self.code)
            .or_else(|| fallback.lookup(&self.source, &self.code))
        {
            self.name = info.name.clone();
            self.description = info.description.clone();
        }
    }

    /// Valor de uma coluna como texto (coluna fixa ou campo dinâmico).
    pub fn col_str(&self, col: &str) -> Option<String> {
        match col {
            "id" => Some(self.id.to_string()),
            "timestamp" => self.timestamp.map(ts_to_iso),
            "source" => Some(self.source.clone()),
            "level" => Some(self.level.clone()),
            "code" => Some(self.code.clone()),
            "name" => Some(self.name.clone()),
            "description" => Some(self.description.clone()),
            "message" => Some(self.message.clone()),
            "raw" => Some(self.raw.clone()),
            other => self.fields.get(other).map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            }),
        }
    }

    /// Valor de uma coluna como texto por referência: colunas fixas e campos
    /// `Value::String` vêm emprestados (sem alocar); `id`/`timestamp` e campos
    /// não-string são materializados. Mesmos valores de `col_str`.
    pub fn col_ref(&self, col: &str) -> Option<Cow<'_, str>> {
        match col {
            "id" => Some(Cow::Owned(self.id.to_string())),
            "timestamp" => self.timestamp.map(|t| Cow::Owned(ts_to_iso(t))),
            "source" => Some(Cow::Borrowed(self.source.as_str())),
            "level" => Some(Cow::Borrowed(self.level.as_str())),
            "code" => Some(Cow::Borrowed(self.code.as_str())),
            "name" => Some(Cow::Borrowed(self.name.as_str())),
            "description" => Some(Cow::Borrowed(self.description.as_str())),
            "message" => Some(Cow::Borrowed(self.message.as_str())),
            "raw" => Some(Cow::Borrowed(self.raw.as_str())),
            other => self.fields.get(other).map(|v| match v {
                Value::String(s) => Cow::Borrowed(s.as_str()),
                other => Cow::Owned(other.to_string()),
            }),
        }
    }

    /// Valor de uma coluna como número (para filtros >, < e agregações).
    pub fn col_num(&self, col: &str) -> Option<f64> {
        match col {
            "id" => Some(self.id as f64),
            "timestamp" => self.timestamp.map(|t| t as f64),
            _ => self.col_str(col).and_then(|s| {
                let s = s.trim();
                // aceita valores com unidade ("20 MB", "120 ms") além de números puros
                s.parse::<f64>()
                    .ok()
                    .or_else(|| crate::analysis::parse_num_unit(s).map(|(n, _)| n))
            }),
        }
    }
}

pub fn ts_to_iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

// ---------------------------------------------------------- níveis (classes)
pub const LV_INFO: u8 = 0;
pub const LV_CRIT: u8 = 1;
pub const LV_ERR: u8 = 2;
pub const LV_WARN: u8 = 3;
pub const LV_DEBUG: u8 = 4;
pub const LV_TRACE: u8 = 5;
pub const LV_OTHER: u8 = 6;

pub fn class_label(c: u8) -> &'static str {
    match c {
        LV_CRIT => "Crítico",
        LV_ERR => "Erro",
        LV_WARN => "Aviso",
        LV_DEBUG => "Depuração",
        LV_TRACE => "Rastreio",
        _ => "Informação",
    }
}

/// Classe de um rótulo canônico ("Erro" → LV_ERR). `None` para rótulos fora do conjunto.
pub fn label_class(label: &str) -> Option<u8> {
    Some(match label {
        "Crítico" => LV_CRIT,
        "Erro" => LV_ERR,
        "Aviso" => LV_WARN,
        "Informação" => LV_INFO,
        "Depuração" => LV_DEBUG,
        "Rastreio" => LV_TRACE,
        _ => return None,
    })
}

// ---------------------------------------------------------- índice de linhas
/// Metadados compactos de uma linha de arquivo (~32 bytes), usados para
/// filtrar/ordenar/agregar sem materializar o evento completo.
#[derive(Clone, Copy, Debug, Default)]
pub struct LineMeta {
    pub offset: u64,
    pub len: u32,
    /// Epoch ms; 0 = sem timestamp reconhecido.
    pub ts: i64,
    pub level: u8,
    /// Fatia do código relativa ao início da linha (`code_len` = 0 → sem código).
    pub code_off: u32,
    pub code_len: u16,
}

impl LineMeta {
    pub fn code<'a>(&self, line: &'a [u8]) -> &'a [u8] {
        let start = self.code_off as usize;
        let end = start + self.code_len as usize;
        if self.code_len == 0 || end > line.len() {
            b""
        } else {
            &line[start..end]
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CodeInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// Mapa de enriquecimento: fonte ("*" = qualquer) -> código -> nome/descrição.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CodesConfig {
    #[serde(flatten)]
    pub sources: HashMap<String, HashMap<String, CodeInfo>>,
}

impl CodesConfig {
    pub fn lookup(&self, source: &str, code: &str) -> Option<&CodeInfo> {
        self.sources
            .get(source)
            .and_then(|m| m.get(code))
            .or_else(|| self.sources.get("*").and_then(|m| m.get(code)))
    }
}
