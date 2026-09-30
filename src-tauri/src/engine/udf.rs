//! Rust functions callable from the engine's SQL. Filters call the app's own
//! comparison code through them, so both engines judge every value alike.
use duckdb::core::{DataChunkHandle, Inserter, LogicalTypeId};
use duckdb::ffi::duckdb_string_t;
use duckdb::types::DuckString;
use duckdb::vscalar::{ScalarFunctionSignature, VScalar};
use duckdb::vtab::arrow::WritableVector;
use duckdb::Connection;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, LazyLock};

pub(crate) type TextTest = Arc<dyn Fn(Option<&str>) -> bool + Send + Sync>;
pub(crate) type NumberTest = Arc<dyn Fn(Option<f64>) -> bool + Send + Sync>;

#[derive(Clone)]
enum Test {
    Text(TextTest),
    Number(NumberTest),
}

static TESTS: LazyLock<RwLock<HashMap<i32, Test>>> = LazyLock::new(Default::default);
static NEXT: AtomicI32 = AtomicI32::new(1);

/// Tests referenced by one statement; they are unregistered on drop.
#[derive(Default)]
pub(crate) struct Tests {
    ids: Vec<i32>,
    pub(crate) free: Vec<FreeText>,
    pub(crate) hex_fields: Vec<HexField>,
}

pub(crate) struct FreeText {
    pub(crate) marker: String,
    pub(crate) needle: String,
    pub(crate) names_sql: String,
}

pub(crate) struct HexField {
    pub(crate) marker: String,
    pub(crate) filter: Arc<crate::query::PreparedFilter>,
    pub(crate) fallback_sql: String,
    pub(crate) word: String,
}

impl Tests {
    fn add(&mut self, test: Test) -> i32 {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        TESTS.write().insert(id, test);
        self.ids.push(id);
        id
    }
    /// SQL calling `test` with the text of `value` (NULL = absent).
    pub(crate) fn text(&mut self, value: &str, test: TextTest) -> String {
        let id = self.add(Test::Text(test));
        format!("li_test({value}, {id})")
    }
    /// SQL calling `test` with the number `value` (NULL = absent).
    pub(crate) fn number(&mut self, value: &str, test: NumberTest) -> String {
        let id = self.add(Test::Number(test));
        format!("li_ntest(CAST({value} AS DOUBLE), {id})")
    }
    /// A structured planner placeholder, expanded before SQL reaches DuckDB.
    /// Keeping the complete free-text term together lets candidate selection
    /// avoid a full enrichment join without dropping name/description matches.
    pub(crate) fn free_text(&mut self, needle: &str) -> String {
        let owned = needle.to_string();
        let test: TextTest = Arc::new(move |v| {
            v.is_some_and(|v| crate::query::ci_contains_bytes(v.as_bytes(), owned.as_bytes()))
        });
        let id = self.add(Test::Text(test));
        let marker = format!("__li_free_{id}()");
        self.free.push(FreeText {
            marker: marker.clone(),
            needle: needle.into(),
            names_sql: format!("(li_test(name, {id}) OR li_test(description, {id}))"),
        });
        marker
    }
    pub(crate) fn hex_field(&mut self, filter: Arc<crate::query::PreparedFilter>, fallback_sql: String, word: String) -> String {
        let marker = format!("__li_hex_{}()", NEXT.fetch_add(1, Ordering::Relaxed));
        self.hex_fields.push(HexField { marker: marker.clone(), filter, fallback_sql, word });
        marker
    }
}

impl Drop for Tests {
    fn drop(&mut self) {
        if self.ids.is_empty() {
            return;
        }
        let mut tests = TESTS.write();
        for id in &self.ids {
            tests.remove(id);
        }
    }
}

fn lookup(id: i32) -> Result<Test, Box<dyn Error>> {
    TESTS
        .read()
        .get(&id)
        .cloned()
        .ok_or_else(|| "Filtro expirado no motor de consultas.".into())
}

/// Calls `f` with a DuckDB string as `&str`. DuckDB keeps VARCHAR valid UTF-8.
fn with_text<R>(value: &duckdb_string_t, f: impl FnOnce(&str) -> R) -> R {
    let mut copy = *value;
    let bytes = DuckString::new(&mut copy).as_bytes();
    match std::str::from_utf8(bytes) {
        Ok(text) => f(text),
        Err(_) => f(&String::from_utf8_lossy(bytes)),
    }
}

fn varchar() -> duckdb::core::LogicalTypeHandle {
    LogicalTypeId::Varchar.into()
}

struct TextTestFn;
impl VScalar for TextTestFn {
    type State = ();
    fn invoke(_: &(), input: &mut DataChunkHandle, output: &mut dyn WritableVector) -> Result<(), Box<dyn Error>> {
        let rows = input.len();
        let values = input.flat_vector(0);
        let ids = input.flat_vector(1);
        let ids = unsafe { ids.as_slice_with_len::<i32>(rows) };
        let texts = unsafe { values.as_slice_with_len::<duckdb_string_t>(rows) };
        let mut out = output.flat_vector();
        let out = unsafe { out.as_mut_slice_with_len::<bool>(rows) };
        let mut current: Option<(i32, TextTest)> = None;
        for row in 0..rows {
            let id = ids[row];
            if current.as_ref().is_none_or(|(c, _)| *c != id) {
                let Test::Text(test) = lookup(id)? else {
                    return Err("Teste numérico usado como texto.".into());
                };
                current = Some((id, test));
            }
            let test = &current.as_ref().expect("test loaded").1;
            out[row] = if values.row_is_null(row as u64) {
                test(None)
            } else {
                with_text(&texts[row], |text| test(Some(text)))
            };
        }
        Ok(())
    }
    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(
            vec![varchar(), LogicalTypeId::Integer.into()],
            LogicalTypeId::Boolean.into(),
        )]
    }
}

struct NumberTestFn;
impl VScalar for NumberTestFn {
    type State = ();
    fn invoke(_: &(), input: &mut DataChunkHandle, output: &mut dyn WritableVector) -> Result<(), Box<dyn Error>> {
        let rows = input.len();
        let values = input.flat_vector(0);
        let ids = input.flat_vector(1);
        let ids = unsafe { ids.as_slice_with_len::<i32>(rows) };
        let numbers = unsafe { values.as_slice_with_len::<f64>(rows) };
        let mut out = output.flat_vector();
        let out = unsafe { out.as_mut_slice_with_len::<bool>(rows) };
        let mut current: Option<(i32, NumberTest)> = None;
        for row in 0..rows {
            let id = ids[row];
            if current.as_ref().is_none_or(|(c, _)| *c != id) {
                let Test::Number(test) = lookup(id)? else {
                    return Err("Teste de texto usado como número.".into());
                };
                current = Some((id, test));
            }
            let test = &current.as_ref().expect("test loaded").1;
            out[row] = test((!values.row_is_null(row as u64)).then_some(numbers[row]));
        }
        Ok(())
    }
    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(
            vec![LogicalTypeId::Double.into(), LogicalTypeId::Integer.into()],
            LogicalTypeId::Boolean.into(),
        )]
    }
}

/// Maps each non-null text to another text; `None` becomes NULL.
fn map_text(input: &mut DataChunkHandle, output: &mut dyn WritableVector, map: impl Fn(&str) -> Option<String>) {
    let rows = input.len();
    let values = input.flat_vector(0);
    let texts = unsafe { values.as_slice_with_len::<duckdb_string_t>(rows) };
    let mut out = output.flat_vector();
    for row in 0..rows {
        let mapped = if values.row_is_null(row as u64) {
            None
        } else {
            with_text(&texts[row], &map)
        };
        match mapped {
            Some(text) => out.insert(row, text.as_str()),
            None => out.set_null(row),
        }
    }
}

/// `str::to_lowercase`, as the app's sort keys use.
struct LowerFn;
impl VScalar for LowerFn {
    type State = ();
    fn invoke(_: &(), input: &mut DataChunkHandle, output: &mut dyn WritableVector) -> Result<(), Box<dyn Error>> {
        map_text(input, output, |text| Some(text.to_lowercase()));
        Ok(())
    }
    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(vec![varchar()], varchar())]
    }
}

/// Text of the timestamp column (`ts_to_iso`).
struct IsoFn;
impl VScalar for IsoFn {
    type State = ();
    fn invoke(_: &(), input: &mut DataChunkHandle, output: &mut dyn WritableVector) -> Result<(), Box<dyn Error>> {
        let rows = input.len();
        let values = input.flat_vector(0);
        let times = unsafe { values.as_slice_with_len::<i64>(rows) };
        let mut out = output.flat_vector();
        for row in 0..rows {
            if values.row_is_null(row as u64) {
                out.set_null(row);
            } else {
                out.insert(row, crate::model::ts_to_iso(times[row]).as_str());
            }
        }
        Ok(())
    }
    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(vec![LogicalTypeId::Bigint.into()], varchar())]
    }
}

/// Writes one optional number per row of a text column.
fn map_number<T: Copy>(input: &mut DataChunkHandle, output: &mut dyn WritableVector, map: impl Fn(&str) -> Option<T>) {
    let rows = input.len();
    let values = input.flat_vector(0);
    let texts = unsafe { values.as_slice_with_len::<duckdb_string_t>(rows) };
    let mut out = output.flat_vector();
    let mut nulls = Vec::new();
    {
        let data = unsafe { out.as_mut_slice_with_len::<T>(rows) };
        for row in 0..rows {
            let value = if values.row_is_null(row as u64) {
                None
            } else {
                with_text(&texts[row], &map)
            };
            match value {
                Some(v) => data[row] = v,
                None => nulls.push(row),
            }
        }
    }
    for row in nulls {
        out.set_null(row);
    }
}

/// Order of `f64::total_cmp` as an integer, for numbers written in a column.
struct NumberKeyFn;
impl VScalar for NumberKeyFn {
    type State = ();
    fn invoke(_: &(), input: &mut DataChunkHandle, output: &mut dyn WritableVector) -> Result<(), Box<dyn Error>> {
        map_number::<i64>(input, output, |text| {
            crate::model::text_number(text).map(|n| {
                let bits = n.to_bits() as i64;
                bits ^ (((bits >> 63) as u64) >> 1) as i64
            })
        });
        Ok(())
    }
    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(vec![varchar()], LogicalTypeId::Bigint.into())]
    }
}

/// Group labels use strict f64 parsing (without trimming or unit conversion),
/// unlike event-column sorting. Keep the cap's tie-break identical to Rust.
struct GroupNumberKeyFn;
impl VScalar for GroupNumberKeyFn {
    type State = ();
    fn invoke(_: &(), input: &mut DataChunkHandle, output: &mut dyn WritableVector) -> Result<(), Box<dyn Error>> {
        map_number::<i64>(input, output, |text| {
            text.parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| {
                let bits = n.to_bits() as i64;
                bits ^ (((bits >> 63) as u64) >> 1) as i64
            })
        });
        Ok(())
    }
    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(vec![varchar()], LogicalTypeId::Bigint.into())]
    }
}

/// Value of `parse_num_unit` (sums and averages).
struct NumFn;
impl VScalar for NumFn {
    type State = ();
    fn invoke(_: &(), input: &mut DataChunkHandle, output: &mut dyn WritableVector) -> Result<(), Box<dyn Error>> {
        map_number::<f64>(input, output, |text| crate::analysis::parse_num_unit(text).map(|(n, _)| n));
        Ok(())
    }
    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(vec![varchar()], LogicalTypeId::Double.into())]
    }
}

/// Unit of `parse_num_unit` (`UnitKind` as a number).
struct UnitFn;
impl VScalar for UnitFn {
    type State = ();
    fn invoke(_: &(), input: &mut DataChunkHandle, output: &mut dyn WritableVector) -> Result<(), Box<dyn Error>> {
        map_number::<i32>(input, output, |text| crate::analysis::parse_num_unit(text).map(|(_, u)| u as i32));
        Ok(())
    }
    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(vec![varchar()], LogicalTypeId::Integer.into())]
    }
}

/// Whether a grouping value counts as empty (absent or only whitespace).
struct BlankFn;
impl VScalar for BlankFn {
    type State = ();
    fn invoke(_: &(), input: &mut DataChunkHandle, output: &mut dyn WritableVector) -> Result<(), Box<dyn Error>> {
        let rows = input.len();
        let values = input.flat_vector(0);
        let texts = unsafe { values.as_slice_with_len::<duckdb_string_t>(rows) };
        let mut out = output.flat_vector();
        let out = unsafe { out.as_mut_slice_with_len::<bool>(rows) };
        for row in 0..rows {
            out[row] = values.row_is_null(row as u64) || with_text(&texts[row], |t| t.trim().is_empty());
        }
        Ok(())
    }
    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(vec![varchar()], LogicalTypeId::Boolean.into())]
    }
}

pub(crate) fn register(conn: &Connection) -> duckdb::Result<()> {
    conn.register_scalar_function::<TextTestFn>("li_test")?;
    conn.register_scalar_function::<NumberTestFn>("li_ntest")?;
    conn.register_scalar_function::<LowerFn>("li_lower")?;
    conn.register_scalar_function::<IsoFn>("li_iso")?;
    conn.register_scalar_function::<NumberKeyFn>("li_nkey")?;
    conn.register_scalar_function::<GroupNumberKeyFn>("li_gkey")?;
    conn.register_scalar_function::<NumFn>("li_num")?;
    conn.register_scalar_function::<UnitFn>("li_unit")?;
    conn.register_scalar_function::<BlankFn>("li_blank")?;
    Ok(())
}
