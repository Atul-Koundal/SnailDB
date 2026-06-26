pub mod catalog;
pub mod executor;
pub mod sql;
pub mod storage;

use executor::Engine;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use sql::lexer::Lexer;
use sql::parser::Parser;

/// Python-facing wrapper around Engine.
#[pyclass]
struct SnailDB {
    engine: Engine,
}

#[pymethods]
impl SnailDB {
    #[new]
    fn new(path: &str) -> PyResult<Self> {
        let engine = Engine::open(path)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        Ok(SnailDB { engine })
    }

    /// Execute a SQL string. Returns a dict with keys:
    ///   - "columns": list of column name strings
    ///   - "rows":    list of lists of strings (or None for NULL)
    ///   - "message": string or None
    fn execute(&self, py: Python<'_>, sql: &str) -> PyResult<PyObject> {
        let tokens = Lexer::new(sql)
            .tokenize()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let stmt = Parser::new(tokens)
            .parse()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let result = self
            .engine
            .execute(stmt)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let dict = PyDict::new(py);

        // columns
        let cols = PyList::new(py, &result.columns)?;
        dict.set_item("columns", cols)?;

        // rows
        let rows = PyList::empty(py);
        for row in &result.rows {
            let py_row = PyList::empty(py);
            for cell in row {
                match cell {
                    Some(s) => py_row.append(s)?,
                    None => py_row.append(py.None())?,
                }
            }
            rows.append(py_row)?;
        }
        dict.set_item("rows", rows)?;

        // message
        match &result.message {
            Some(m) => dict.set_item("message", m)?,
            None => dict.set_item("message", py.None())?,
        }

        Ok(dict.into())
    }
}

/// The Python module name must match the crate name.
#[pymodule]
fn snail_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<SnailDB>()?;
    Ok(())
}