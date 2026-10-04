//! Bounded Arrow chunks for large DuckDB outputs. The row API materializes an
//! entire result; it must not be used to transport the full fact population.
use duckdb::arrow::{
    array::{Array, BooleanArray, Float64Array, Int32Array, Int64Array, StringArray},
    record_batch::RecordBatch,
};
use std::sync::Arc;
pub struct Rows<'s> {
    stream: duckdb::ArrowStream<'s>,
    batch: Option<Arc<RecordBatch>>,
    index: usize,
}
pub struct Row {
    batch: Arc<RecordBatch>,
    index: usize,
}
impl<'s> Rows<'s> {
    pub fn new(statement: &'s mut duckdb::Statement<'_>) -> Result<Self, String> {
        Ok(Self {
            stream: statement.stream_arrow([]).map_err(|e| e.to_string())?,
            batch: None,
            index: 0,
        })
    }
    pub fn next(&mut self) -> Result<Option<Row>, String> {
        loop {
            crate::operations::check()?;
            crate::security_budget::check()?;
            if let Some(batch) = &self.batch {
                if self.index < batch.num_rows() {
                    let row = Row {
                        batch: batch.clone(),
                        index: self.index,
                    };
                    self.index += 1;
                    return Ok(Some(row));
                }
            }
            let next = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.stream.next()
            }))
            .map_err(|panic| {
                crate::security_budget::failure().unwrap_or_else(|| {
                    panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| {
                            "Falha ao ler o lote colunar; nenhum resultado parcial foi publicado"
                                .into()
                        })
                })
            })?;
            let Some(batch) = next else { return Ok(None) };
            self.batch = Some(Arc::new(batch));
            self.index = 0;
        }
    }
}
pub trait FromArrow: Sized {
    fn from_arrow(array: &dyn Array, index: usize) -> Result<Self, String>;
}
impl Row {
    pub fn get<T: FromArrow>(&self, column: usize) -> Result<T, String> {
        let array = self
            .batch
            .columns()
            .get(column)
            .ok_or("Coluna ausente no lote colunar")?;
        T::from_arrow(array.as_ref(), self.index)
    }
}
impl FromArrow for String {
    fn from_arrow(array: &dyn Array, index: usize) -> Result<Self, String> {
        if array.is_null(index) {
            return Err("Texto obrigatório ausente no lote colunar".into());
        }
        array
            .as_any()
            .downcast_ref::<StringArray>()
            .map(|a| a.value(index).to_string())
            .ok_or_else(|| "Tipo de texto incompatível no lote colunar".into())
    }
}
impl FromArrow for i64 {
    fn from_arrow(array: &dyn Array, index: usize) -> Result<Self, String> {
        if array.is_null(index) {
            return Err("Inteiro obrigatório ausente no lote colunar".into());
        }
        if let Some(a) = array.as_any().downcast_ref::<Int64Array>() {
            Ok(a.value(index))
        } else if let Some(a) = array.as_any().downcast_ref::<Int32Array>() {
            Ok(a.value(index) as i64)
        } else {
            Err("Tipo inteiro incompatível no lote colunar".into())
        }
    }
}
impl FromArrow for i32 {
    fn from_arrow(array: &dyn Array, index: usize) -> Result<Self, String> {
        i32::try_from(i64::from_arrow(array, index)?)
            .map_err(|_| "Inteiro excede o intervalo no lote colunar".into())
    }
}
impl FromArrow for bool {
    fn from_arrow(array: &dyn Array, index: usize) -> Result<Self, String> {
        if array.is_null(index) {
            return Err("Booleano obrigatório ausente no lote colunar".into());
        }
        array
            .as_any()
            .downcast_ref::<BooleanArray>()
            .map(|a| a.value(index))
            .ok_or_else(|| "Tipo booleano incompatível no lote colunar".into())
    }
}
impl FromArrow for f64 {
    fn from_arrow(array: &dyn Array, index: usize) -> Result<Self, String> {
        if array.is_null(index) {
            return Err("Número obrigatório ausente no lote colunar".into());
        }
        array
            .as_any()
            .downcast_ref::<Float64Array>()
            .map(|a| a.value(index))
            .ok_or_else(|| "Tipo numérico incompatível no lote colunar".into())
    }
}
impl<T: FromArrow> FromArrow for Option<T> {
    fn from_arrow(array: &dyn Array, index: usize) -> Result<Self, String> {
        if array.is_null(index) {
            Ok(None)
        } else {
            T::from_arrow(array, index).map(Some)
        }
    }
}
