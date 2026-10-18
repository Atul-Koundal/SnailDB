use crate::catalog::{Catalog, ColType, Column, Schema};
use crate::sql::ast::*;
use crate::storage::Storage;
use serde_json::{Map, Value as JsonValue};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),
    #[error("catalog error: {0}")]
    Catalog(#[from] crate::catalog::CatalogError),
    #[error("serialise error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("table '{0}' does not exist")]
    TableNotFound(String),
    #[error("column '{0}' does not exist in table '{1}'")]
    ColumnNotFound(String, String),
    #[error("wrong number of values: expected {expected}, got {got}")]
    ColumnCountMismatch { expected: usize, got: usize },
    #[error("column '{0}' must appear in GROUP BY or be used in an aggregate function")]
    InvalidAggregate(String),
    #[error("integer overflow while computing {0}")]
    Overflow(String),
    #[error("type mismatch for column '{col}': expected {expected}, got {got}")]
    TypeMismatch {
        col: String,
        expected: String,
        got: String,
    },
}

pub type Result<T> = std::result::Result<T, ExecError>;

#[derive(Debug)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub message: Option<String>,
}

impl QueryResult {
    fn message(msg: &str) -> Self {
        QueryResult {
            columns: vec![],
            rows: vec![],
            message: Some(msg.to_string()),
        }
    }
}

pub struct Engine {
    storage: Storage,
}

impl Engine {
    pub fn open(path: &str) -> Result<Self> {
        let storage = Storage::open(path)?;
        Ok(Engine { storage })
    }

    pub fn execute(&self, stmt: Stmt) -> Result<QueryResult> {
        match stmt {
            Stmt::CreateTable(s) => self.exec_create(s),
            Stmt::Insert(s) => self.exec_insert(s),
            Stmt::Select(s) => self.exec_select(s),
            Stmt::Update(s) => self.exec_update(s),
            Stmt::Delete(s) => self.exec_delete(s),
        }
    }

    fn exec_create(&self, stmt: CreateTableStmt) -> Result<QueryResult> {
        let catalog = Catalog::new(&self.storage);
        let columns = stmt
            .columns
            .into_iter()
            .map(|c| Column {
                name: c.name,
                col_type: c.col_type,
            })
            .collect();
        let schema = Schema::new(&stmt.table_name, columns);
        catalog.create_table(schema)?;
        Ok(QueryResult::message(&format!(
            "Table '{}' created.",
            stmt.table_name
        )))
    }

    fn exec_insert(&self, stmt: InsertStmt) -> Result<QueryResult> {
        let catalog = Catalog::new(&self.storage);
        let schema = catalog
            .get_schema(&stmt.table_name)
            .map_err(|_| ExecError::TableNotFound(stmt.table_name.clone()))?;

        for row_vals in &stmt.rows {
            if row_vals.len() != stmt.columns.len() {
                return Err(ExecError::ColumnCountMismatch {
                    expected: stmt.columns.len(),
                    got: row_vals.len(),
                });
            }
        }
        for col_name in &stmt.columns {
            if schema.column_index(col_name).is_none() {
                return Err(ExecError::ColumnNotFound(
                    col_name.to_string(),
                    stmt.table_name.clone(),
                ));
            }
        }

        let mut inserted = 0usize;
        for row_vals in stmt.rows {
            let mut map = Map::new();
            for (col_name, val) in stmt.columns.iter().zip(row_vals.iter()) {
                let schema_col = schema.columns.iter().find(|c| &c.name == col_name).unwrap();
                let json_val = coerce_value(val, &schema_col.col_type, col_name)?;
                map.insert(col_name.to_string(), json_val);
            }
            let row_id = self.next_row_id(&stmt.table_name)?;
            let key = row_key(&stmt.table_name, row_id);
            let bytes = serde_json::to_vec(&JsonValue::Object(map))?;
            self.storage.put(&key, &bytes)?;
            inserted += 1;
        }
        Ok(QueryResult::message(&format!(
            "{} row(s) inserted.",
            inserted
        )))
    }

    fn exec_select(&self, stmt: SelectStmt) -> Result<QueryResult> {
        let catalog = Catalog::new(&self.storage);

        // ── Scan left (base) table ────────────────────────────────────
        let left_schema = catalog
            .get_schema(&stmt.table_name)
            .map_err(|_| ExecError::TableNotFound(stmt.table_name.clone()))?;

        let prefix = format!("row:{}:", stmt.table_name);
        let raw_left = self.storage.scan_prefix(prefix.as_bytes())?;

        // Each "combined row" is a flat map of "table.column" -> value
        // This lets us handle both qualified and unqualified column refs.
        let mut combined_rows: Vec<Map<String, JsonValue>> = raw_left
            .into_iter()
            .map(|(_, bytes)| {
                let obj: Map<String, JsonValue> = serde_json::from_slice(&bytes).unwrap();
                qualify_row(&stmt.table_name, obj)
            })
            .collect();

        // Schemas of every table in the query (base + joined), used to validate columns.
        let mut all_schemas = vec![left_schema.clone()];
        for join in &stmt.joins {
            all_schemas.push(
                catalog
                    .get_schema(&join.table_name)
                    .map_err(|_| ExecError::TableNotFound(join.table_name.clone()))?,
            );
        }

        // ── Process each JOIN ─────────────────────────────────────────
        for join in &stmt.joins {
            let right_prefix = format!("row:{}:", join.table_name);
            let raw_right = self.storage.scan_prefix(right_prefix.as_bytes())?;
            let right_rows: Vec<Map<String, JsonValue>> = raw_right
                .into_iter()
                .map(|(_, bytes)| {
                    let obj: Map<String, JsonValue> = serde_json::from_slice(&bytes).unwrap();
                    qualify_row(&join.table_name, obj)
                })
                .collect();

            let left_key = format!("{}.{}", join.on.left_table, join.on.left_col);
            let right_key = format!("{}.{}", join.on.right_table, join.on.right_col);

            let mut new_rows: Vec<Map<String, JsonValue>> = Vec::new();

            for left_row in &combined_rows {
                let left_val = left_row.get(&left_key);
                let mut matched = false;

                for right_row in &right_rows {
                    let right_val = right_row.get(&right_key);
                    if left_val == right_val && left_val.is_some() {
                        // Merge left + right into one combined row
                        let mut merged = left_row.clone();
                        for (k, v) in right_row {
                            merged.insert(k.clone(), v.clone());
                        }
                        new_rows.push(merged);
                        matched = true;
                    }
                }

                // LEFT JOIN: keep left row even with no match
                if !matched && join.join_type == JoinType::Left {
                    let mut merged = left_row.clone();
                    // Fill right-side columns with Null
                    let right_schema = catalog
                        .get_schema(&join.table_name)
                        .map_err(|_| ExecError::TableNotFound(join.table_name.clone()))?;
                    for col in &right_schema.columns {
                        let qkey = format!("{}.{}", join.table_name, col.name);
                        merged.insert(qkey, JsonValue::Null);
                    }
                    new_rows.push(merged);
                }
            }
            combined_rows = new_rows;
        }

        // ── WHERE filter ──────────────────────────────────────────────
        if let Some(expr) = &stmt.where_clause {
            combined_rows.retain(|row| eval_expr(expr, row));
        }

        // ── Aggregates ────────────────────────────────────────────────
        if let SelectColumns::Items(items) = &stmt.columns {
            return exec_aggregate(items, &combined_rows, &all_schemas, stmt.limit);
        }

        // ── ORDER BY ──────────────────────────────────────────────────
        if !stmt.order_by.is_empty() {
            combined_rows.sort_by(|a, b| {
                for clause in &stmt.order_by {
                    let av = lookup_column(a, &clause.column);
                    let bv = lookup_column(b, &clause.column);
                    let ord = compare_json_values(av, bv);
                    let ord = if clause.direction == OrderDirection::Desc {
                        ord.reverse()
                    } else {
                        ord
                    };
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                }
                std::cmp::Ordering::Equal
            });
        }

        // ── LIMIT ─────────────────────────────────────────────────────
        if let Some(n) = stmt.limit {
            combined_rows.truncate(n);
        }

        // ── Project output columns ────────────────────────────────────
        let has_joins = !stmt.joins.is_empty();

        let out_col_names: Vec<String> = match &stmt.columns {
            SelectColumns::Star => {
                if has_joins {
                    // For joins, use qualified names from the first row
                    combined_rows
                        .first()
                        .map(|r| r.keys().cloned().collect())
                        .unwrap_or_default()
                } else {
                    left_schema
                        .columns
                        .iter()
                        .map(|c| format!("{}.{}", stmt.table_name, c.name))
                        .collect()
                }
            }
            SelectColumns::Items(_) => unreachable!("aggregate queries return earlier"),
            SelectColumns::Named(refs) => refs
                .iter()
                .map(|r| {
                    match &r.table {
                        Some(t) => format!("{}.{}", t, r.column),
                        None => {
                            // Unqualified — find which table has this column
                            if let Some(row) = combined_rows.first() {
                                let found =
                                    row.keys().find(|k| k.ends_with(&format!(".{}", r.column)));
                                found.cloned().unwrap_or_else(|| r.column.clone())
                            } else {
                                r.column.clone()
                            }
                        }
                    }
                })
                .collect(),
        };

        // Display names strip the "table." prefix for cleaner output
        let display_names: Vec<String> = match &stmt.columns {
            SelectColumns::Star if !has_joins => {
                left_schema.columns.iter().map(|c| c.name.clone()).collect()
            }
            SelectColumns::Named(refs) => refs.iter().map(|r| r.display_name()).collect(),
            SelectColumns::Items(_) => unreachable!("aggregate queries return earlier"),
            _ => out_col_names.clone(),
        };

        let rows = combined_rows
            .iter()
            .map(|obj| {
                out_col_names
                    .iter()
                    .map(|col| {
                        obj.get(col).map(|v| match v {
                            JsonValue::Number(n) => n.to_string(),
                            JsonValue::String(s) => s.clone(),
                            JsonValue::Null => "NULL".to_string(),
                            other => other.to_string(),
                        })
                    })
                    .collect()
            })
            .collect();

        Ok(QueryResult {
            columns: display_names,
            rows,
            message: None,
        })
    }

    fn exec_update(&self, stmt: UpdateStmt) -> Result<QueryResult> {
        let catalog = Catalog::new(&self.storage);
        let schema = catalog
            .get_schema(&stmt.table_name)
            .map_err(|_| ExecError::TableNotFound(stmt.table_name.clone()))?;

        for assignment in &stmt.assignments {
            if schema.column_index(&assignment.column).is_none() {
                return Err(ExecError::ColumnNotFound(
                    assignment.column.clone(),
                    stmt.table_name.clone(),
                ));
            }
        }

        let prefix = format!("row:{}:", stmt.table_name);
        let raw_rows = self.storage.scan_prefix(prefix.as_bytes())?;
        let mut updated = 0usize;

        for (key, bytes) in raw_rows {
            let mut obj: Map<String, JsonValue> = serde_json::from_slice(&bytes)?;
            if let Some(expr) = &stmt.where_clause {
                if !eval_expr(expr, &obj) {
                    continue;
                }
            }
            for assignment in &stmt.assignments {
                let schema_col = schema
                    .columns
                    .iter()
                    .find(|c| c.name == assignment.column)
                    .unwrap();
                let json_val =
                    coerce_value(&assignment.value, &schema_col.col_type, &assignment.column)?;
                obj.insert(assignment.column.clone(), json_val);
            }
            let new_bytes = serde_json::to_vec(&JsonValue::Object(obj))?;
            self.storage.put(&key, &new_bytes)?;
            updated += 1;
        }
        Ok(QueryResult::message(&format!(
            "{} row(s) updated.",
            updated
        )))
    }

    fn exec_delete(&self, stmt: DeleteStmt) -> Result<QueryResult> {
        let catalog = Catalog::new(&self.storage);
        catalog
            .get_schema(&stmt.table_name)
            .map_err(|_| ExecError::TableNotFound(stmt.table_name.clone()))?;

        let prefix = format!("row:{}:", stmt.table_name);
        let raw_rows = self.storage.scan_prefix(prefix.as_bytes())?;
        let mut deleted = 0usize;

        for (key, bytes) in raw_rows {
            let obj: Map<String, JsonValue> = serde_json::from_slice(&bytes)?;
            if let Some(expr) = &stmt.where_clause {
                if !eval_expr(expr, &obj) {
                    continue;
                }
            }
            self.storage.delete(&key)?;
            deleted += 1;
        }
        Ok(QueryResult::message(&format!(
            "{} row(s) deleted.",
            deleted
        )))
    }

    fn next_row_id(&self, table_name: &str) -> Result<u64> {
        let key = format!("meta:{}:next_id", table_name);
        let id = match self.storage.get(key.as_bytes())? {
            Some(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                s.parse::<u64>().unwrap_or(1)
            }
            None => 1,
        };
        self.storage
            .put(key.as_bytes(), (id + 1).to_string().as_bytes())?;
        Ok(id)
    }
} // closes impl Engine

// ── Key helpers ───────────────────────────────────────────────────────────────

fn row_key(table_name: &str, id: u64) -> Vec<u8> {
    format!("row:{}:{:020}", table_name, id).into_bytes()
}

/// Prefix all keys in a row map with "table_name." so JOIN rows
/// can hold columns from multiple tables without key collisions.
fn qualify_row(table_name: &str, row: Map<String, JsonValue>) -> Map<String, JsonValue> {
    row.into_iter()
        .map(|(k, v)| (format!("{}.{}", table_name, k), v))
        .collect()
}

// ── Value coercion ────────────────────────────────────────────────────────────

fn coerce_value(val: &Value, col_type: &ColType, col_name: &str) -> Result<JsonValue> {
    match (val, col_type) {
        (Value::Integer(n), ColType::Integer) => Ok(JsonValue::Number((*n).into())),
        (Value::Text(s), ColType::Text) => Ok(JsonValue::String(s.clone())),
        (Value::Null, _) => Ok(JsonValue::Null),
        (Value::Integer(n), ColType::Text) => Ok(JsonValue::String(n.to_string())),
        (Value::Text(s), ColType::Integer) => s
            .parse::<i64>()
            .map(|n| JsonValue::Number(n.into()))
            .map_err(|_| ExecError::TypeMismatch {
                col: col_name.to_string(),
                expected: "INTEGER".to_string(),
                got: format!("'{}'", s),
            }),
    }
}

// ── Column lookup ─────────────────────────────────────────────────────────────

/// Find a column in a row by name. Accepts a qualified key (`users.age`) or a bare
/// column name (`age`), which matches the first `<table>.age` key.
fn lookup_column<'a>(row: &'a Map<String, JsonValue>, name: &str) -> Option<&'a JsonValue> {
    row.get(name).or_else(|| {
        let suffix = format!(".{}", name);
        row.keys()
            .find(|k| k.ends_with(&suffix))
            .and_then(|k| row.get(k))
    })
}

fn lookup_ref<'a>(row: &'a Map<String, JsonValue>, col: &ColumnRef) -> Option<&'a JsonValue> {
    match &col.table {
        Some(t) => row.get(&format!("{}.{}", t, col.column)),
        None => lookup_column(row, &col.column),
    }
}

/// Error unless `col` exists in at least one of the query's tables.
fn validate_column(col: &ColumnRef, schemas: &[Schema]) -> Result<()> {
    let found = schemas.iter().any(|s| {
        col.table.as_deref().is_none_or(|t| s.table_name == t)
            && s.column_index(&col.column).is_some()
    });
    if found {
        Ok(())
    } else {
        let tables: Vec<&str> = schemas.iter().map(|s| s.table_name.as_str()).collect();
        Err(ExecError::ColumnNotFound(
            col.display_name(),
            tables.join(", "),
        ))
    }
}

// ── Aggregates ────────────────────────────────────────────────────────────────

/// Evaluate a select list of aggregates over all `rows`, producing a single result row.
fn exec_aggregate(
    items: &[SelectItem],
    rows: &[Map<String, JsonValue>],
    schemas: &[Schema],
    limit: Option<usize>,
) -> Result<QueryResult> {
    let mut columns = Vec::new();
    let mut cells = Vec::new();
    for item in items {
        match item {
            SelectItem::Column(c) => {
                return Err(ExecError::InvalidAggregate(c.display_name()));
            }
            SelectItem::Aggregate(agg) => {
                if let AggArg::Column(c) = &agg.arg {
                    validate_column(c, schemas)?;
                }
                columns.push(agg.display_name());
                cells.push(Some(compute_aggregate(agg, rows)?));
            }
        }
    }
    let mut out_rows = vec![cells];
    if limit == Some(0) {
        out_rows.clear();
    }
    Ok(QueryResult {
        columns,
        rows: out_rows,
        message: None,
    })
}

/// Compute one aggregate over a set of rows. NULLs are ignored (SQL semantics),
/// except by `COUNT(*)`. An aggregate over no non-NULL values is `NULL`
/// (`COUNT` is `0`).
fn compute_aggregate(agg: &AggregateExpr, rows: &[Map<String, JsonValue>]) -> Result<String> {
    let col = match &agg.arg {
        AggArg::Star => return Ok(rows.len().to_string()),
        AggArg::Column(c) => c,
    };
    let values: Vec<&JsonValue> = rows
        .iter()
        .filter_map(|r| lookup_ref(r, col))
        .filter(|v| !v.is_null())
        .collect();

    match agg.func {
        AggFunc::Count => Ok(values.len().to_string()),
        AggFunc::Sum | AggFunc::Avg => {
            let mut sum: i64 = 0;
            for v in &values {
                let n = v.as_i64().ok_or_else(|| ExecError::TypeMismatch {
                    col: agg.display_name(),
                    expected: "INTEGER".to_string(),
                    got: "TEXT".to_string(),
                })?;
                sum = sum
                    .checked_add(n)
                    .ok_or_else(|| ExecError::Overflow(agg.display_name()))?;
            }
            if values.is_empty() {
                Ok("NULL".to_string())
            } else if agg.func == AggFunc::Sum {
                Ok(sum.to_string())
            } else {
                Ok(format_float(sum as f64 / values.len() as f64))
            }
        }
        AggFunc::Min | AggFunc::Max => {
            let best = values.into_iter().reduce(|a, b| {
                let ord = compare_json_values(Some(a), Some(b));
                let keep_a = if agg.func == AggFunc::Min {
                    ord != std::cmp::Ordering::Greater
                } else {
                    ord != std::cmp::Ordering::Less
                };
                if keep_a {
                    a
                } else {
                    b
                }
            });
            Ok(match best {
                Some(JsonValue::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => "NULL".to_string(),
            })
        }
    }
}

/// `30.0` rather than `30`, so an AVG is visibly not an integer column.
fn format_float(f: f64) -> String {
    if f.fract() == 0.0 {
        format!("{:.1}", f)
    } else {
        f.to_string()
    }
}

// ── WHERE evaluator ───────────────────────────────────────────────────────────

fn eval_expr(expr: &Expr, row: &Map<String, JsonValue>) -> bool {
    match expr {
        Expr::And(l, r) => eval_expr(l, row) && eval_expr(r, row),
        Expr::Or(l, r) => eval_expr(l, row) || eval_expr(r, row),
        Expr::Comparison { column, op, value } => {
            // Try both qualified (table.col) and unqualified (col) lookup
            let row_val = lookup_column(row, column);
            match row_val {
                Some(v) => compare(v, op, value),
                None => false,
            }
        }
    }
}

fn compare(row_val: &JsonValue, op: &CmpOp, filter_val: &Value) -> bool {
    match (row_val, filter_val) {
        (JsonValue::Number(n), Value::Integer(i)) => {
            let row_n = n.as_i64().unwrap_or(0);
            match op {
                CmpOp::Eq => row_n == *i,
                CmpOp::NotEq => row_n != *i,
                CmpOp::Lt => row_n < *i,
                CmpOp::Lte => row_n <= *i,
                CmpOp::Gt => row_n > *i,
                CmpOp::Gte => row_n >= *i,
            }
        }
        (JsonValue::String(s), Value::Text(t)) => match op {
            CmpOp::Eq => s == t,
            CmpOp::NotEq => s != t,
            CmpOp::Lt => s < t,
            CmpOp::Lte => s <= t,
            CmpOp::Gt => s > t,
            CmpOp::Gte => s >= t,
        },
        _ => false,
    }
}

fn compare_json_values(a: Option<&JsonValue>, b: Option<&JsonValue>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(JsonValue::Number(an)), Some(JsonValue::Number(bn))) => {
            let ai = an.as_f64().unwrap_or(0.0);
            let bi = bn.as_f64().unwrap_or(0.0);
            ai.partial_cmp(&bi).unwrap_or(std::cmp::Ordering::Equal)
        }
        (Some(JsonValue::String(as_)), Some(JsonValue::String(bs))) => as_.cmp(bs),
        (None, None) => std::cmp::Ordering::Equal,
        (None, _) => std::cmp::Ordering::Less,
        (_, None) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::lexer::Lexer;
    use crate::sql::parser::Parser;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_engine() -> (Engine, std::path::PathBuf) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("snaildb_exec_{}_{}.redb", ts, n));
        let engine = Engine::open(path.to_str().unwrap()).unwrap();
        (engine, path)
    }

    fn run(engine: &Engine, sql: &str) -> QueryResult {
        let tokens = Lexer::new(sql).tokenize().unwrap();
        let stmt = Parser::new(tokens).parse().unwrap();
        engine.execute(stmt).unwrap()
    }

    #[test]
    fn create_table() {
        let (engine, p) = temp_engine();
        let result = run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        assert!(result.message.unwrap().contains("created"));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn insert_and_select_star() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(&engine, "INSERT INTO users (id, name) VALUES (1, 'Alice')");
        run(&engine, "INSERT INTO users (id, name) VALUES (2, 'Bob')");
        let result = run(&engine, "SELECT * FROM users");
        assert_eq!(result.rows.len(), 2);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn select_named_columns() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(&engine, "INSERT INTO users (id, name) VALUES (1, 'Alice')");
        let result = run(&engine, "SELECT name FROM users");
        assert_eq!(result.columns, vec!["name"]);
        assert_eq!(result.rows[0][0], Some("Alice".to_string()));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn where_integer_filter() {
        let (engine, p) = temp_engine();
        run(
            &engine,
            "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (2, 'Bob', 20)",
        );
        let result = run(&engine, "SELECT * FROM users WHERE age > 25");
        assert_eq!(result.rows.len(), 1);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn where_text_filter() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(&engine, "INSERT INTO users (id, name) VALUES (1, 'Alice')");
        run(&engine, "INSERT INTO users (id, name) VALUES (2, 'Bob')");
        let result = run(&engine, "SELECT * FROM users WHERE name = 'Alice'");
        assert_eq!(result.rows.len(), 1);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn where_and_filter() {
        let (engine, p) = temp_engine();
        run(
            &engine,
            "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (2, 'Bob', 30)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (3, 'Alice', 20)",
        );
        let result = run(
            &engine,
            "SELECT * FROM users WHERE name = 'Alice' AND age = 30",
        );
        assert_eq!(result.rows.len(), 1);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn where_or_filter() {
        let (engine, p) = temp_engine();
        run(
            &engine,
            "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (2, 'Bob', 20)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (3, 'Carol', 25)",
        );
        let result = run(&engine, "SELECT * FROM users WHERE age = 30 OR age = 20");
        assert_eq!(result.rows.len(), 2);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn multi_row_insert() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(
            &engine,
            "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Carol')",
        );
        let result = run(&engine, "SELECT * FROM users");
        assert_eq!(result.rows.len(), 3);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn update_row() {
        let (engine, p) = temp_engine();
        run(
            &engine,
            "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)",
        );
        run(&engine, "UPDATE users SET age = 31 WHERE id = 1");
        let result = run(&engine, "SELECT * FROM users WHERE id = 1");
        assert_eq!(result.rows[0][2], Some("31".to_string()));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn delete_row() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(&engine, "INSERT INTO users (id, name) VALUES (1, 'Alice')");
        run(&engine, "INSERT INTO users (id, name) VALUES (2, 'Bob')");
        run(&engine, "DELETE FROM users WHERE id = 1");
        let result = run(&engine, "SELECT * FROM users");
        assert_eq!(result.rows.len(), 1);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn delete_all_rows() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(&engine, "INSERT INTO users (id, name) VALUES (1, 'Alice')");
        run(&engine, "INSERT INTO users (id, name) VALUES (2, 'Bob')");
        run(&engine, "DELETE FROM users");
        let result = run(&engine, "SELECT * FROM users");
        assert_eq!(result.rows.len(), 0);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn order_by_asc() {
        let (engine, p) = temp_engine();
        run(
            &engine,
            "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (2, 'Bob', 20)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (3, 'Carol', 25)",
        );
        let result = run(&engine, "SELECT * FROM users ORDER BY age ASC");
        assert_eq!(result.rows[0][2], Some("20".to_string()));
        assert_eq!(result.rows[2][2], Some("30".to_string()));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn order_by_desc() {
        let (engine, p) = temp_engine();
        run(
            &engine,
            "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (2, 'Bob', 20)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (3, 'Carol', 25)",
        );
        let result = run(&engine, "SELECT * FROM users ORDER BY age DESC");
        assert_eq!(result.rows[0][2], Some("30".to_string()));
        assert_eq!(result.rows[2][2], Some("20".to_string()));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn limit() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(
            &engine,
            "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Carol')",
        );
        let result = run(&engine, "SELECT * FROM users LIMIT 2");
        assert_eq!(result.rows.len(), 2);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn order_by_and_limit() {
        let (engine, p) = temp_engine();
        run(
            &engine,
            "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (2, 'Bob', 20)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (3, 'Carol', 25)",
        );
        let result = run(&engine, "SELECT * FROM users ORDER BY age DESC LIMIT 2");
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][2], Some("30".to_string()));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn inner_join() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(
            &engine,
            "CREATE TABLE orders (id INTEGER, user_id INTEGER, amount INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob')",
        );
        run(
            &engine,
            "INSERT INTO orders (id, user_id, amount) VALUES (1, 1, 100), (2, 1, 200), (3, 2, 50)",
        );

        let result = run(&engine,
            "SELECT users.name, orders.amount FROM users INNER JOIN orders ON users.id = orders.user_id"
        );
        assert_eq!(result.rows.len(), 3);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn inner_join_filters_non_matching() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(
            &engine,
            "CREATE TABLE orders (id INTEGER, user_id INTEGER, amount INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Carol')",
        );
        run(
            &engine,
            "INSERT INTO orders (id, user_id, amount) VALUES (1, 1, 100)",
        );

        let result = run(&engine,
            "SELECT users.name, orders.amount FROM users INNER JOIN orders ON users.id = orders.user_id"
        );
        // Only Alice has an order
        assert_eq!(result.rows.len(), 1);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn left_join_keeps_non_matching() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(
            &engine,
            "CREATE TABLE orders (id INTEGER, user_id INTEGER, amount INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob')",
        );
        run(
            &engine,
            "INSERT INTO orders (id, user_id, amount) VALUES (1, 1, 100)",
        );

        let result = run(&engine,
            "SELECT users.name, orders.amount FROM users LEFT JOIN orders ON users.id = orders.user_id"
        );
        // Both users returned, Bob's amount is NULL
        assert_eq!(result.rows.len(), 2);
        let bob_row = result
            .rows
            .iter()
            .find(|r| r[0] == Some("Bob".to_string()))
            .unwrap();
        assert_eq!(bob_row[1], Some("NULL".to_string()));
        let _ = std::fs::remove_file(p);
    }

    fn run_err(engine: &Engine, sql: &str) -> ExecError {
        let tokens = Lexer::new(sql).tokenize().unwrap();
        let stmt = Parser::new(tokens).parse().unwrap();
        engine.execute(stmt).unwrap_err()
    }

    fn people(engine: &Engine) {
        run(
            engine,
            "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)",
        );
        run(
            engine,
            "INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30), (2, 'Bob', 20), (3, 'Carol', 25), (4, 'Dave', NULL)",
        );
    }

    fn cell(result: &QueryResult, col: usize) -> String {
        result.rows[0][col].clone().unwrap()
    }

    #[test]
    fn count_star_counts_all_rows() {
        let (engine, p) = temp_engine();
        people(&engine);
        let result = run(&engine, "SELECT COUNT(*) FROM users");
        assert_eq!(result.columns, vec!["COUNT(*)"]);
        assert_eq!(cell(&result, 0), "4");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn count_column_ignores_nulls() {
        let (engine, p) = temp_engine();
        people(&engine);
        let result = run(&engine, "SELECT COUNT(age) FROM users");
        assert_eq!(cell(&result, 0), "3");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn sum_avg_min_max() {
        let (engine, p) = temp_engine();
        people(&engine);
        let result = run(
            &engine,
            "SELECT SUM(age), AVG(age), MIN(age), MAX(age) FROM users",
        );
        assert_eq!(
            result.columns,
            vec!["SUM(age)", "AVG(age)", "MIN(age)", "MAX(age)"]
        );
        assert_eq!(cell(&result, 0), "75");
        assert_eq!(cell(&result, 1), "25.0");
        assert_eq!(cell(&result, 2), "20");
        assert_eq!(cell(&result, 3), "30");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn avg_with_fraction() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE t (n INTEGER)");
        run(&engine, "INSERT INTO t (n) VALUES (1), (2)");
        let result = run(&engine, "SELECT AVG(n) FROM t");
        assert_eq!(cell(&result, 0), "1.5");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn min_max_on_text() {
        let (engine, p) = temp_engine();
        people(&engine);
        let result = run(&engine, "SELECT MIN(name), MAX(name) FROM users");
        assert_eq!(cell(&result, 0), "Alice");
        assert_eq!(cell(&result, 1), "Dave");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn aggregate_respects_where() {
        let (engine, p) = temp_engine();
        people(&engine);
        let result = run(
            &engine,
            "SELECT COUNT(*), SUM(age) FROM users WHERE age > 20",
        );
        assert_eq!(cell(&result, 0), "2");
        assert_eq!(cell(&result, 1), "55");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn aggregates_on_empty_table() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE t (n INTEGER)");
        let result = run(
            &engine,
            "SELECT COUNT(*), SUM(n), AVG(n), MIN(n), MAX(n) FROM t",
        );
        assert_eq!(result.rows.len(), 1);
        assert_eq!(cell(&result, 0), "0");
        for i in 1..5 {
            assert_eq!(cell(&result, i), "NULL");
        }
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn aggregate_over_join() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(
            &engine,
            "CREATE TABLE orders (id INTEGER, user_id INTEGER, amount INTEGER)",
        );
        run(
            &engine,
            "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob')",
        );
        run(
            &engine,
            "INSERT INTO orders (id, user_id, amount) VALUES (1, 1, 100), (2, 1, 200), (3, 2, 50)",
        );
        let result = run(
            &engine,
            "SELECT COUNT(*), SUM(orders.amount) FROM users INNER JOIN orders ON users.id = orders.user_id",
        );
        assert_eq!(cell(&result, 0), "3");
        assert_eq!(cell(&result, 1), "350");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn plain_column_with_aggregate_is_an_error() {
        let (engine, p) = temp_engine();
        people(&engine);
        let err = run_err(&engine, "SELECT name, COUNT(*) FROM users");
        assert!(matches!(err, ExecError::InvalidAggregate(_)));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn sum_on_text_column_is_an_error() {
        let (engine, p) = temp_engine();
        people(&engine);
        let err = run_err(&engine, "SELECT SUM(name) FROM users");
        assert!(matches!(err, ExecError::TypeMismatch { .. }));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn aggregate_on_unknown_column_is_an_error() {
        let (engine, p) = temp_engine();
        people(&engine);
        let err = run_err(&engine, "SELECT MAX(salary) FROM users");
        assert!(matches!(err, ExecError::ColumnNotFound(_, _)));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn join_with_missing_table_is_an_error() {
        let (engine, p) = temp_engine();
        people(&engine);
        let err = run_err(
            &engine,
            "SELECT users.name FROM users INNER JOIN ghosts ON users.id = ghosts.user_id",
        );
        assert!(matches!(err, ExecError::TableNotFound(_)));
        let _ = std::fs::remove_file(p);
    }
}
