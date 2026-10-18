use crate::catalog::{Catalog, ColType, Column, IndexDef, Schema};
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
    #[error("a transaction is already active; COMMIT or ROLLBACK first")]
    TransactionActive,
    #[error("no active transaction")]
    NoTransaction,
    #[error("index '{0}' already exists")]
    IndexExists(String),
    #[error("column '{col}' of table '{table}' is already indexed")]
    ColumnAlreadyIndexed { table: String, col: String },
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
            Stmt::Begin => {
                if !self.storage.begin() {
                    return Err(ExecError::TransactionActive);
                }
                Ok(QueryResult::message("Transaction started."))
            }
            Stmt::Commit => {
                if !self.storage.commit()? {
                    return Err(ExecError::NoTransaction);
                }
                Ok(QueryResult::message("Transaction committed."))
            }
            Stmt::Rollback => {
                if !self.storage.rollback() {
                    return Err(ExecError::NoTransaction);
                }
                Ok(QueryResult::message("Transaction rolled back."))
            }
            Stmt::Select(s) => self.exec_select(s),
            Stmt::Explain(inner) => self.exec_explain(*inner),
            Stmt::ShowTables => self.exec_show_tables(),
            Stmt::Describe(table) => self.exec_describe(&table),
            write => self.execute_atomically(write),
        }
    }

    /// Run a write statement so that it either fully applies or leaves no trace.
    ///
    /// Outside a transaction the statement gets an implicit one (one durable commit
    /// instead of one per row). Inside an explicit transaction, a failing statement
    /// is undone but the transaction stays open, like in most SQL databases.
    fn execute_atomically(&self, stmt: Stmt) -> Result<QueryResult> {
        let before = self.storage.snapshot();
        let implicit = before.is_none();
        if implicit {
            self.storage.begin();
        }
        let result = match stmt {
            Stmt::CreateTable(s) => self.exec_create(s),
            Stmt::CreateIndex(s) => self.exec_create_index(s),
            Stmt::Insert(s) => self.exec_insert(s),
            Stmt::Update(s) => self.exec_update(s),
            Stmt::Delete(s) => self.exec_delete(s),
            Stmt::Select(_)
            | Stmt::Begin
            | Stmt::Commit
            | Stmt::Rollback
            | Stmt::Explain(_)
            | Stmt::ShowTables
            | Stmt::Describe(_) => unreachable!("handled in execute"),
        };
        match result {
            Ok(r) => {
                if implicit {
                    self.storage.commit()?;
                }
                Ok(r)
            }
            Err(e) => {
                self.storage.restore(before);
                Err(e)
            }
        }
    }

    pub fn in_transaction(&self) -> bool {
        self.storage.in_transaction()
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
            let bytes = serde_json::to_vec(&JsonValue::Object(map.clone()))?;
            self.storage.put(&key, &bytes)?;
            self.put_index_entries(&schema, row_id, &map)?;
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

        // An index can narrow the base-table scan. With joins the WHERE columns may belong
        // to either table, so joins always use a full scan.
        let index_where = if stmt.joins.is_empty() {
            stmt.where_clause.as_ref()
        } else {
            None
        };
        let raw_left = self.scan_rows(&stmt.table_name, &left_schema, index_where)?;

        // Each "combined row" is a flat map of "table.column" -> value
        // This lets us handle both qualified and unqualified column refs.
        let mut combined_rows: Vec<Map<String, JsonValue>> = raw_left
            .into_iter()
            .map(|(_, bytes)| {
                let obj: Map<String, JsonValue> = serde_json::from_slice(&bytes)?;
                Ok(qualify_row(&stmt.table_name, obj))
            })
            .collect::<Result<_>>()?;

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
                    let obj: Map<String, JsonValue> = serde_json::from_slice(&bytes)?;
                    Ok(qualify_row(&join.table_name, obj))
                })
                .collect::<Result<_>>()?;

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

        // ── Aggregates / GROUP BY / HAVING ────────────────────────────
        let is_grouped = !stmt.group_by.is_empty()
            || stmt.having.is_some()
            || matches!(stmt.columns, SelectColumns::Items(_));
        if is_grouped {
            let items: Vec<SelectItem> = match &stmt.columns {
                SelectColumns::Items(items) => items.clone(),
                SelectColumns::Named(refs) => {
                    refs.iter().cloned().map(SelectItem::Column).collect()
                }
                SelectColumns::Star => return Err(ExecError::InvalidAggregate("*".to_string())),
            };
            return exec_grouped(
                &items,
                &stmt.group_by,
                stmt.having.as_ref(),
                &stmt.order_by,
                stmt.limit,
                &combined_rows,
                &all_schemas,
            );
        }

        // ── ORDER BY ──────────────────────────────────────────────────
        if let Some(bad) = stmt.order_by.iter().find(|c| c.column.contains('(')) {
            return Err(ExecError::InvalidAggregate(bad.column.clone()));
        }
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

        let raw_rows = self.scan_rows(&stmt.table_name, &schema, stmt.where_clause.as_ref())?;
        let mut updated = 0usize;

        for (key, bytes) in raw_rows {
            let mut obj: Map<String, JsonValue> = serde_json::from_slice(&bytes)?;
            let old_obj = obj.clone();
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
            let new_bytes = serde_json::to_vec(&JsonValue::Object(obj.clone()))?;
            self.storage.put(&key, &new_bytes)?;
            self.reindex_row(&schema, row_id_from_key(&key), &old_obj, &obj)?;
            updated += 1;
        }
        Ok(QueryResult::message(&format!(
            "{} row(s) updated.",
            updated
        )))
    }

    fn exec_delete(&self, stmt: DeleteStmt) -> Result<QueryResult> {
        let catalog = Catalog::new(&self.storage);
        let schema = catalog
            .get_schema(&stmt.table_name)
            .map_err(|_| ExecError::TableNotFound(stmt.table_name.clone()))?;

        let raw_rows = self.scan_rows(&stmt.table_name, &schema, stmt.where_clause.as_ref())?;
        let mut deleted = 0usize;

        for (key, bytes) in raw_rows {
            let obj: Map<String, JsonValue> = serde_json::from_slice(&bytes)?;
            if let Some(expr) = &stmt.where_clause {
                if !eval_expr(expr, &obj) {
                    continue;
                }
            }
            self.storage.delete(&key)?;
            self.delete_index_entries(&schema, row_id_from_key(&key), &obj)?;
            deleted += 1;
        }
        Ok(QueryResult::message(&format!(
            "{} row(s) deleted.",
            deleted
        )))
    }

    fn exec_show_tables(&self) -> Result<QueryResult> {
        let mut tables = Catalog::new(&self.storage).list_tables()?;
        tables.sort();
        Ok(QueryResult {
            columns: vec!["table".to_string()],
            rows: tables.into_iter().map(|t| vec![Some(t)]).collect(),
            message: None,
        })
    }

    fn exec_describe(&self, table: &str) -> Result<QueryResult> {
        let schema = Catalog::new(&self.storage)
            .get_schema(table)
            .map_err(|_| ExecError::TableNotFound(table.to_string()))?;
        let rows = schema
            .columns
            .iter()
            .map(|c| {
                let index = schema
                    .indexes
                    .iter()
                    .find(|i| i.column == c.name)
                    .map(|i| i.name.clone())
                    .unwrap_or_default();
                vec![
                    Some(c.name.clone()),
                    Some(c.col_type.name().to_string()),
                    Some(index),
                ]
            })
            .collect();
        Ok(QueryResult {
            columns: vec!["column".into(), "type".into(), "index".into()],
            rows,
            message: None,
        })
    }

    /// Describe how a statement would run, without running it.
    fn exec_explain(&self, stmt: Stmt) -> Result<QueryResult> {
        let catalog = Catalog::new(&self.storage);
        let get = |t: &str| {
            catalog
                .get_schema(t)
                .map_err(|_| ExecError::TableNotFound(t.to_string()))
        };
        let mut plan: Vec<String> = Vec::new();

        match &stmt {
            Stmt::Select(s) => {
                let schema = get(&s.table_name)?;
                if s.joins.is_empty() {
                    plan.push(scan_line(&s.table_name, &schema, s.where_clause.as_ref()));
                } else {
                    plan.push(format!(
                        "FULL SCAN {} (joins disable index use)",
                        s.table_name
                    ));
                }
                for j in &s.joins {
                    get(&j.table_name)?;
                    let kind = if j.join_type == JoinType::Left {
                        "LEFT"
                    } else {
                        "INNER"
                    };
                    plan.push(format!(
                        "{} JOIN {} ON {}.{} = {}.{} (nested loop)",
                        kind,
                        j.table_name,
                        j.on.left_table,
                        j.on.left_col,
                        j.on.right_table,
                        j.on.right_col
                    ));
                }
                if let Some(w) = &s.where_clause {
                    plan.push(format!("FILTER {}", fmt_expr(w, true)));
                }
                let has_agg = matches!(s.columns, SelectColumns::Items(_));
                if !s.group_by.is_empty() {
                    let cols: Vec<String> = s.group_by.iter().map(|c| c.display_name()).collect();
                    plan.push(format!("GROUP BY {}", cols.join(", ")));
                } else if has_agg {
                    plan.push("AGGREGATE (single group)".to_string());
                }
                if let Some(h) = &s.having {
                    plan.push(format!("HAVING {}", fmt_having(h, true)));
                }
                if !s.order_by.is_empty() {
                    let parts: Vec<String> = s
                        .order_by
                        .iter()
                        .map(|o| {
                            let dir = if o.direction == OrderDirection::Desc {
                                "DESC"
                            } else {
                                "ASC"
                            };
                            format!("{} {}", o.column, dir)
                        })
                        .collect();
                    plan.push(format!("SORT BY {}", parts.join(", ")));
                }
                if let Some(n) = s.limit {
                    plan.push(format!("LIMIT {}", n));
                }
            }
            Stmt::Update(u) => {
                let schema = get(&u.table_name)?;
                plan.push(scan_line(&u.table_name, &schema, u.where_clause.as_ref()));
                if let Some(w) = &u.where_clause {
                    plan.push(format!("FILTER {}", fmt_expr(w, true)));
                }
                let cols: Vec<&str> = u.assignments.iter().map(|a| a.column.as_str()).collect();
                plan.push(format!("UPDATE columns: {}", cols.join(", ")));
            }
            Stmt::Delete(d) => {
                let schema = get(&d.table_name)?;
                plan.push(scan_line(&d.table_name, &schema, d.where_clause.as_ref()));
                if let Some(w) = &d.where_clause {
                    plan.push(format!("FILTER {}", fmt_expr(w, true)));
                }
                plan.push("DELETE matching rows".to_string());
            }
            _ => unreachable!("the parser only builds EXPLAIN for SELECT/UPDATE/DELETE"),
        }

        Ok(QueryResult {
            columns: vec!["plan".to_string()],
            rows: plan.into_iter().map(|l| vec![Some(l)]).collect(),
            message: None,
        })
    }

    fn exec_create_index(&self, stmt: CreateIndexStmt) -> Result<QueryResult> {
        let catalog = Catalog::new(&self.storage);
        let mut schema = catalog
            .get_schema(&stmt.table_name)
            .map_err(|_| ExecError::TableNotFound(stmt.table_name.clone()))?;

        if schema.column_index(&stmt.column).is_none() {
            return Err(ExecError::ColumnNotFound(
                stmt.column.clone(),
                stmt.table_name.clone(),
            ));
        }
        // Index names are unique across the whole database.
        for table in catalog.list_tables()? {
            if catalog
                .get_schema(&table)?
                .indexes
                .iter()
                .any(|i| i.name == stmt.index_name)
            {
                return Err(ExecError::IndexExists(stmt.index_name));
            }
        }
        if schema.indexes.iter().any(|i| i.column == stmt.column) {
            return Err(ExecError::ColumnAlreadyIndexed {
                table: stmt.table_name,
                col: stmt.column,
            });
        }

        // Backfill entries for rows that already exist.
        let prefix = format!("row:{}:", stmt.table_name);
        let mut indexed = 0usize;
        for (key, bytes) in self.storage.scan_prefix(prefix.as_bytes())? {
            let obj: Map<String, JsonValue> = serde_json::from_slice(&bytes)?;
            let entry = index_entry_key(
                &stmt.table_name,
                &stmt.column,
                &encode_index_json(obj.get(&stmt.column)),
                row_id_from_key(&key),
            );
            self.storage.put(&entry, b"")?;
            indexed += 1;
        }

        schema.indexes.push(IndexDef {
            name: stmt.index_name.clone(),
            column: stmt.column.clone(),
        });
        catalog.save_schema(&schema)?;
        Ok(QueryResult::message(&format!(
            "Index '{}' created on {}({}), {} row(s) indexed.",
            stmt.index_name, stmt.table_name, stmt.column, indexed
        )))
    }

    // ── Index maintenance ─────────────────────────────────────────────

    fn put_index_entries(
        &self,
        schema: &Schema,
        row_id: u64,
        row: &Map<String, JsonValue>,
    ) -> Result<()> {
        for idx in &schema.indexes {
            let enc = encode_index_json(row.get(&idx.column));
            let key = index_entry_key(&schema.table_name, &idx.column, &enc, row_id);
            self.storage.put(&key, b"")?;
        }
        Ok(())
    }

    fn delete_index_entries(
        &self,
        schema: &Schema,
        row_id: u64,
        row: &Map<String, JsonValue>,
    ) -> Result<()> {
        for idx in &schema.indexes {
            let enc = encode_index_json(row.get(&idx.column));
            let key = index_entry_key(&schema.table_name, &idx.column, &enc, row_id);
            self.storage.delete(&key)?;
        }
        Ok(())
    }

    /// After an UPDATE: move index entries whose indexed value changed.
    fn reindex_row(
        &self,
        schema: &Schema,
        row_id: u64,
        old: &Map<String, JsonValue>,
        new: &Map<String, JsonValue>,
    ) -> Result<()> {
        for idx in &schema.indexes {
            let old_enc = encode_index_json(old.get(&idx.column));
            let new_enc = encode_index_json(new.get(&idx.column));
            if old_enc != new_enc {
                let table = &schema.table_name;
                self.storage
                    .delete(&index_entry_key(table, &idx.column, &old_enc, row_id))?;
                self.storage
                    .put(&index_entry_key(table, &idx.column, &new_enc, row_id), b"")?;
            }
        }
        Ok(())
    }

    // ── Row access ────────────────────────────────────────────────────

    /// Rows of `table` that may satisfy `where_clause`, as `(row key, JSON bytes)` in
    /// row-id order. Uses an index when one applies, otherwise a full scan. Callers
    /// still evaluate the full WHERE expression, so an index only ever narrows the
    /// candidate set and never changes results.
    fn scan_rows(
        &self,
        table: &str,
        schema: &Schema,
        where_clause: Option<&Expr>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if let Some(lookup) = where_clause.and_then(|w| choose_index(schema, w)) {
            let mut rows = Vec::new();
            for key in self.index_candidate_keys(table, &lookup)? {
                if let Some(bytes) = self.storage.get(&key)? {
                    rows.push((key, bytes));
                }
            }
            return Ok(rows);
        }
        let prefix = format!("row:{}:", table);
        Ok(self.storage.scan_prefix(prefix.as_bytes())?)
    }

    /// Row keys whose indexed value satisfies the lookup, sorted by row id so results
    /// come back in the same order as a full scan.
    fn index_candidate_keys(&self, table: &str, lookup: &IndexLookup) -> Result<Vec<Vec<u8>>> {
        let base = index_prefix(table, &lookup.index.column);
        let enc = encode_index_value(&lookup.value).unwrap_or_default();
        let tag = &enc[..1];

        let entries = match lookup.op {
            CmpOp::Eq => self
                .storage
                .scan_prefix(format!("{}{}{}", base, enc, SEP).as_bytes())?,
            CmpOp::Gt => self.storage.scan_range(
                format!("{}{}{}", base, enc, AFTER_SEP).as_bytes(),
                format!("{}{}{}", base, tag, TAG_END).as_bytes(),
            )?,
            CmpOp::Gte => self.storage.scan_range(
                format!("{}{}{}", base, enc, SEP).as_bytes(),
                format!("{}{}{}", base, tag, TAG_END).as_bytes(),
            )?,
            CmpOp::Lt => self.storage.scan_range(
                format!("{}{}", base, tag).as_bytes(),
                format!("{}{}{}", base, enc, SEP).as_bytes(),
            )?,
            CmpOp::Lte => self.storage.scan_range(
                format!("{}{}", base, tag).as_bytes(),
                format!("{}{}{}", base, enc, AFTER_SEP).as_bytes(),
            )?,
            CmpOp::NotEq => unreachable!("choose_index never selects !="),
        };

        let mut ids: Vec<u64> = entries.iter().map(|(k, _)| row_id_from_key(k)).collect();
        ids.sort_unstable();
        ids.dedup();
        Ok(ids.into_iter().map(|id| row_key(table, id)).collect())
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

// ── EXPLAIN formatting ────────────────────────────────────────────────────────

fn fmt_op(op: &CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "=",
        CmpOp::NotEq => "!=",
        CmpOp::Lt => "<",
        CmpOp::Lte => "<=",
        CmpOp::Gt => ">",
        CmpOp::Gte => ">=",
    }
}

fn fmt_value(v: &Value) -> String {
    match v {
        Value::Integer(n) => n.to_string(),
        Value::Text(s) => format!("'{}'", s),
        Value::Null => "NULL".to_string(),
    }
}

fn fmt_expr(expr: &Expr, top: bool) -> String {
    match expr {
        Expr::Comparison { column, op, value } => {
            format!("{} {} {}", column, fmt_op(op), fmt_value(value))
        }
        Expr::And(l, r) => wrap(
            format!("{} AND {}", fmt_expr(l, false), fmt_expr(r, false)),
            top,
        ),
        Expr::Or(l, r) => wrap(
            format!("{} OR {}", fmt_expr(l, false), fmt_expr(r, false)),
            top,
        ),
    }
}

fn fmt_having(expr: &HavingExpr, top: bool) -> String {
    match expr {
        HavingExpr::Comparison { agg, op, value } => {
            format!("{} {} {}", agg.display_name(), fmt_op(op), fmt_value(value))
        }
        HavingExpr::And(l, r) => wrap(
            format!("{} AND {}", fmt_having(l, false), fmt_having(r, false)),
            top,
        ),
        HavingExpr::Or(l, r) => wrap(
            format!("{} OR {}", fmt_having(l, false), fmt_having(r, false)),
            top,
        ),
    }
}

fn wrap(s: String, top: bool) -> String {
    if top {
        s
    } else {
        format!("({})", s)
    }
}

/// First line of a plan: which access path the table scan uses.
fn scan_line(table: &str, schema: &Schema, where_clause: Option<&Expr>) -> String {
    match where_clause.and_then(|w| choose_index(schema, w)) {
        Some(l) => format!(
            "INDEX SCAN {} using {} ({} {} {})",
            table,
            l.index.name,
            l.index.column,
            fmt_op(&l.op),
            fmt_value(&l.value)
        ),
        None => format!("FULL SCAN {}", table),
    }
}

// ── Index keys and planning ───────────────────────────────────────────────────
//
// Index entry key:  idx:<table>:<column>:<encoded value>!<row id, 20 digits>
//
// The encoded value is order-preserving: integers are `i` + 16 hex digits of the
// sign-flipped value, text is `t` + hex of its bytes, NULL is `n`. The separator `!`
// (0x21) sorts below every hex digit, so entries order by value, then by row id.

const SEP: char = '!';
/// Sorts directly after every `<enc>!<id>` entry of the same value.
const AFTER_SEP: char = '"';
/// Sorts after every encoded value of one type tag.
const TAG_END: char = '~';

fn index_prefix(table: &str, column: &str) -> String {
    format!("idx:{}:{}:", table, column)
}

fn index_entry_key(table: &str, column: &str, enc: &str, row_id: u64) -> Vec<u8> {
    format!(
        "{}{}{}{:020}",
        index_prefix(table, column),
        enc,
        SEP,
        row_id
    )
    .into_bytes()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn encode_int(n: i64) -> String {
    format!("i{:016x}", (n as u64) ^ (1u64 << 63))
}

/// Encode a stored JSON value (a missing column counts as NULL).
fn encode_index_json(v: Option<&JsonValue>) -> String {
    match v {
        Some(JsonValue::Number(n)) if n.is_i64() => encode_int(n.as_i64().unwrap()),
        Some(JsonValue::String(s)) => format!("t{}", hex(s.as_bytes())),
        _ => "n".to_string(),
    }
}

/// Encode a literal from a WHERE clause. NULL has no encoding: comparisons with NULL
/// never match, so they never use an index.
fn encode_index_value(v: &Value) -> Option<String> {
    match v {
        Value::Integer(n) => Some(encode_int(*n)),
        Value::Text(s) => Some(format!("t{}", hex(s.as_bytes()))),
        Value::Null => None,
    }
}

fn row_id_from_key(key: &[u8]) -> u64 {
    let s = String::from_utf8_lossy(key);
    s.rsplit([':', SEP])
        .next()
        .and_then(|id| id.parse().ok())
        .unwrap_or(0)
}

/// A WHERE condition that an index can answer.
#[derive(Debug, Clone, PartialEq)]
struct IndexLookup {
    index: IndexDef,
    op: CmpOp,
    value: Value,
}

/// Collect the AND-ed conditions of an expression. Anything under an OR is left out:
/// an index cannot answer a disjunction on its own.
fn conjuncts<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match expr {
        Expr::And(l, r) => {
            conjuncts(l, out);
            conjuncts(r, out);
        }
        Expr::Or(_, _) => {}
        cmp => out.push(cmp),
    }
}

/// Pick an index to answer (part of) a WHERE clause. Prefers equality over ranges.
/// `!=` and NULL comparisons are never indexable.
fn choose_index(schema: &Schema, where_clause: &Expr) -> Option<IndexLookup> {
    let mut parts = Vec::new();
    conjuncts(where_clause, &mut parts);

    let mut best: Option<IndexLookup> = None;
    for part in parts {
        if let Expr::Comparison { column, op, value } = part {
            if *op == CmpOp::NotEq || *value == Value::Null {
                continue;
            }
            if let Some(index) = schema.indexes.iter().find(|i| &i.column == column) {
                let candidate = IndexLookup {
                    index: index.clone(),
                    op: op.clone(),
                    value: value.clone(),
                };
                if candidate.op == CmpOp::Eq {
                    return Some(candidate);
                }
                best.get_or_insert(candidate);
            }
        }
    }
    best
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

// ── Aggregates and grouping ───────────────────────────────────────────────────

/// A computed cell value. Keeping values typed until the very end lets `HAVING` and
/// `ORDER BY` compare numbers as numbers instead of comparing their text.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Int(i64),
    Float(f64),
    Text(String),
    Null,
}

impl Cell {
    fn from_json(v: &JsonValue) -> Cell {
        match v {
            JsonValue::Number(n) => match n.as_i64() {
                Some(i) => Cell::Int(i),
                None => Cell::Float(n.as_f64().unwrap_or(0.0)),
            },
            JsonValue::String(s) => Cell::Text(s.clone()),
            JsonValue::Null => Cell::Null,
            other => Cell::Text(other.to_string()),
        }
    }

    fn render(&self) -> String {
        match self {
            Cell::Int(i) => i.to_string(),
            Cell::Float(f) => format_float(*f),
            Cell::Text(s) => s.clone(),
            Cell::Null => "NULL".to_string(),
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Cell::Int(i) => Some(*i as f64),
            Cell::Float(f) => Some(*f),
            _ => None,
        }
    }
}

/// Total ordering used by ORDER BY on computed rows: NULL first, numbers numerically,
/// text lexically.
fn compare_cells(a: &Cell, b: &Cell) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Cell::Null, Cell::Null) => Ordering::Equal,
        (Cell::Null, _) => Ordering::Less,
        (_, Cell::Null) => Ordering::Greater,
        (Cell::Text(x), Cell::Text(y)) => x.cmp(y),
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            _ => Ordering::Equal,
        },
    }
}

fn cmp_matches(ord: std::cmp::Ordering, op: &CmpOp) -> bool {
    use std::cmp::Ordering;
    match op {
        CmpOp::Eq => ord == Ordering::Equal,
        CmpOp::NotEq => ord != Ordering::Equal,
        CmpOp::Lt => ord == Ordering::Less,
        CmpOp::Lte => ord != Ordering::Greater,
        CmpOp::Gt => ord == Ordering::Greater,
        CmpOp::Gte => ord != Ordering::Less,
    }
}

/// Evaluate a HAVING condition against one group of rows.
fn eval_having(expr: &HavingExpr, group: &[&Map<String, JsonValue>]) -> Result<bool> {
    match expr {
        HavingExpr::And(l, r) => Ok(eval_having(l, group)? && eval_having(r, group)?),
        HavingExpr::Or(l, r) => Ok(eval_having(l, group)? || eval_having(r, group)?),
        HavingExpr::Comparison { agg, op, value } => {
            let lhs = compute_aggregate(agg, group)?;
            let rhs = match value {
                Value::Integer(i) => Cell::Int(*i),
                Value::Text(t) => Cell::Text(t.clone()),
                Value::Null => Cell::Null,
            };
            // Comparisons with NULL are never true, as in SQL.
            if lhs == Cell::Null || rhs == Cell::Null {
                return Ok(false);
            }
            let comparable = matches!(
                (&lhs, &rhs),
                (Cell::Text(_), Cell::Text(_)) | (Cell::Int(_) | Cell::Float(_), Cell::Int(_))
            );
            Ok(comparable && cmp_matches(compare_cells(&lhs, &rhs), op))
        }
    }
}

/// Is `c` allowed in the select list of a grouped query (i.e. is it a GROUP BY column)?
fn is_grouped_column(c: &ColumnRef, group_by: &[ColumnRef]) -> bool {
    group_by.iter().any(|g| {
        g.column == c.column && (g.table == c.table || g.table.is_none() || c.table.is_none())
    })
}

/// Execute a query with aggregates and/or GROUP BY / HAVING over already-filtered rows.
///
/// With no GROUP BY all rows form a single group (so `SELECT COUNT(*) FROM empty` yields
/// one row). With GROUP BY there is one group per distinct key, in first-seen order, and
/// an empty input yields no rows.
fn exec_grouped(
    items: &[SelectItem],
    group_by: &[ColumnRef],
    having: Option<&HavingExpr>,
    order_by: &[OrderByClause],
    limit: Option<usize>,
    rows: &[Map<String, JsonValue>],
    schemas: &[Schema],
) -> Result<QueryResult> {
    // ── Validate ──────────────────────────────────────────────────
    for g in group_by {
        validate_column(g, schemas)?;
    }
    for item in items {
        match item {
            SelectItem::Column(c) => {
                validate_column(c, schemas)?;
                if !is_grouped_column(c, group_by) {
                    return Err(ExecError::InvalidAggregate(c.display_name()));
                }
            }
            SelectItem::Aggregate(a) => {
                if let AggArg::Column(c) = &a.arg {
                    validate_column(c, schemas)?;
                }
            }
        }
    }
    if let Some(h) = having {
        validate_having(h, schemas)?;
    }

    // ── Group ─────────────────────────────────────────────────────
    let mut groups: Vec<Vec<&Map<String, JsonValue>>> = Vec::new();
    if group_by.is_empty() {
        groups.push(rows.iter().collect());
    } else {
        let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for row in rows {
            let key_vals: Vec<&JsonValue> = group_by
                .iter()
                .map(|g| lookup_ref(row, g).unwrap_or(&JsonValue::Null))
                .collect();
            let key = serde_json::to_string(&key_vals)?;
            let slot = *index.entry(key).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[slot].push(row);
        }
    }

    // ── Compute one output row per group ──────────────────────────
    let columns: Vec<String> = items
        .iter()
        .map(|i| match i {
            SelectItem::Column(c) => c.display_name(),
            SelectItem::Aggregate(a) => a.display_name(),
        })
        .collect();

    let mut out: Vec<Vec<Cell>> = Vec::new();
    for group in &groups {
        if let Some(h) = having {
            if !eval_having(h, group)? {
                continue;
            }
        }
        let mut cells = Vec::with_capacity(items.len());
        for item in items {
            cells.push(match item {
                SelectItem::Column(c) => group
                    .first()
                    .and_then(|r| lookup_ref(r, c))
                    .map(Cell::from_json)
                    .unwrap_or(Cell::Null),
                SelectItem::Aggregate(a) => compute_aggregate(a, group)?,
            });
        }
        out.push(cells);
    }

    // ── ORDER BY (against output headers) ─────────────────────────
    if !order_by.is_empty() {
        let mut keys = Vec::new();
        for clause in order_by {
            let idx = columns
                .iter()
                .position(|h| h == &clause.column || h.rsplit('.').next() == Some(&clause.column))
                .ok_or_else(|| {
                    ExecError::ColumnNotFound(clause.column.clone(), "the select list".to_string())
                })?;
            keys.push((idx, clause.direction == OrderDirection::Desc));
        }
        out.sort_by(|a, b| {
            for (idx, desc) in &keys {
                let ord = compare_cells(&a[*idx], &b[*idx]);
                let ord = if *desc { ord.reverse() } else { ord };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        });
    }

    // ── LIMIT ─────────────────────────────────────────────────────
    if let Some(n) = limit {
        out.truncate(n);
    }

    let rows = out
        .into_iter()
        .map(|r| r.iter().map(|c| Some(c.render())).collect())
        .collect();
    Ok(QueryResult {
        columns,
        rows,
        message: None,
    })
}

fn validate_having(expr: &HavingExpr, schemas: &[Schema]) -> Result<()> {
    match expr {
        HavingExpr::And(l, r) | HavingExpr::Or(l, r) => {
            validate_having(l, schemas)?;
            validate_having(r, schemas)
        }
        HavingExpr::Comparison { agg, .. } => match &agg.arg {
            AggArg::Column(c) => validate_column(c, schemas),
            AggArg::Star => Ok(()),
        },
    }
}

/// Compute one aggregate over a group of rows. NULLs are ignored (SQL semantics),
/// except by `COUNT(*)`. An aggregate over no non-NULL values is `NULL`
/// (`COUNT` is `0`).
fn compute_aggregate(agg: &AggregateExpr, rows: &[&Map<String, JsonValue>]) -> Result<Cell> {
    let col = match &agg.arg {
        AggArg::Star => return Ok(Cell::Int(rows.len() as i64)),
        AggArg::Column(c) => c,
    };
    let values: Vec<&JsonValue> = rows
        .iter()
        .filter_map(|r| lookup_ref(r, col))
        .filter(|v| !v.is_null())
        .collect();

    match agg.func {
        AggFunc::Count => Ok(Cell::Int(values.len() as i64)),
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
                Ok(Cell::Null)
            } else if agg.func == AggFunc::Sum {
                Ok(Cell::Int(sum))
            } else {
                Ok(Cell::Float(sum as f64 / values.len() as f64))
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
            Ok(best.map(Cell::from_json).unwrap_or(Cell::Null))
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

    fn sales(engine: &Engine) {
        run(
            engine,
            "CREATE TABLE sales (id INTEGER, region TEXT, product TEXT, amount INTEGER)",
        );
        run(
            engine,
            "INSERT INTO sales (id, region, product, amount) VALUES \
             (1, 'north', 'pen', 10), (2, 'north', 'pen', 20), (3, 'north', 'ink', 5), \
             (4, 'south', 'pen', 7), (5, 'south', 'ink', 3), (6, 'east', 'pen', 100)",
        );
    }

    fn col(result: &QueryResult, idx: usize) -> Vec<String> {
        result
            .rows
            .iter()
            .map(|r| r[idx].clone().unwrap())
            .collect()
    }

    #[test]
    fn group_by_count_per_group_in_first_seen_order() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let result = run(
            &engine,
            "SELECT region, COUNT(*) FROM sales GROUP BY region",
        );
        assert_eq!(result.columns, vec!["region", "COUNT(*)"]);
        assert_eq!(col(&result, 0), vec!["north", "south", "east"]);
        assert_eq!(col(&result, 1), vec!["3", "2", "1"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn group_by_sum_and_avg() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let result = run(
            &engine,
            "SELECT region, SUM(amount), AVG(amount) FROM sales GROUP BY region ORDER BY region ASC",
        );
        assert_eq!(col(&result, 0), vec!["east", "north", "south"]);
        assert_eq!(col(&result, 1), vec!["100", "35", "10"]);
        assert_eq!(col(&result, 2), vec!["100.0", "11.666666666666666", "5.0"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn group_by_multiple_columns() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let result = run(
            &engine,
            "SELECT region, product, COUNT(*) FROM sales GROUP BY region, product",
        );
        assert_eq!(result.rows.len(), 5);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn group_by_respects_where() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let result = run(
            &engine,
            "SELECT region, COUNT(*) FROM sales WHERE amount > 5 GROUP BY region ORDER BY region ASC",
        );
        assert_eq!(col(&result, 0), vec!["east", "north", "south"]);
        assert_eq!(col(&result, 1), vec!["1", "2", "1"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn having_filters_groups() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let result = run(
            &engine,
            "SELECT region, COUNT(*) FROM sales GROUP BY region HAVING COUNT(*) > 1 ORDER BY region ASC",
        );
        assert_eq!(col(&result, 0), vec!["north", "south"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn having_compares_numbers_numerically() {
        // "35" < "5" as text, so a string comparison would get this wrong.
        let (engine, p) = temp_engine();
        sales(&engine);
        let result = run(
            &engine,
            "SELECT region FROM sales GROUP BY region HAVING SUM(amount) > 9 ORDER BY region ASC",
        );
        assert_eq!(col(&result, 0), vec!["east", "north", "south"]);
        let result = run(
            &engine,
            "SELECT region FROM sales GROUP BY region HAVING SUM(amount) >= 35",
        );
        assert_eq!(col(&result, 0), vec!["north", "east"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn having_with_and_or() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let result = run(
            &engine,
            "SELECT region FROM sales GROUP BY region \
             HAVING COUNT(*) = 1 OR SUM(amount) > 30 AND COUNT(*) > 2 ORDER BY region ASC",
        );
        // east: COUNT=1; north: SUM 35 > 30 AND COUNT 3 > 2; south: neither
        assert_eq!(col(&result, 0), vec!["east", "north"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn order_by_aggregate_desc_with_limit() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let result = run(
            &engine,
            "SELECT region, SUM(amount) FROM sales GROUP BY region ORDER BY SUM(amount) DESC LIMIT 2",
        );
        assert_eq!(col(&result, 0), vec!["east", "north"]);
        assert_eq!(col(&result, 1), vec!["100", "35"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn nulls_form_their_own_group() {
        let (engine, p) = temp_engine();
        people(&engine);
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (5, 'Eve', NULL)",
        );
        let result = run(
            &engine,
            "SELECT age, COUNT(*) FROM users GROUP BY age ORDER BY age ASC",
        );
        // NULL sorts first and holds Dave and Eve
        assert_eq!(col(&result, 0)[0], "NULL");
        assert_eq!(col(&result, 1)[0], "2");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn group_by_on_empty_table_returns_no_rows() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE t (a INTEGER)");
        let result = run(&engine, "SELECT a, COUNT(*) FROM t GROUP BY a");
        assert!(result.rows.is_empty());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn group_by_over_join() {
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
            "SELECT users.name, SUM(orders.amount) FROM users INNER JOIN orders \
             ON users.id = orders.user_id GROUP BY users.name ORDER BY users.name ASC",
        );
        assert_eq!(result.columns, vec!["users.name", "SUM(orders.amount)"]);
        assert_eq!(col(&result, 0), vec!["Alice", "Bob"]);
        assert_eq!(col(&result, 1), vec!["300", "50"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn having_without_group_by_applies_to_whole_table() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let result = run(&engine, "SELECT COUNT(*) FROM sales HAVING COUNT(*) > 100");
        assert!(result.rows.is_empty());
        let result = run(&engine, "SELECT COUNT(*) FROM sales HAVING COUNT(*) > 1");
        assert_eq!(col(&result, 0), vec!["6"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn selecting_a_non_grouped_column_is_an_error() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let err = run_err(
            &engine,
            "SELECT product, COUNT(*) FROM sales GROUP BY region",
        );
        assert!(matches!(err, ExecError::InvalidAggregate(_)));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn select_star_with_group_by_is_an_error() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let err = run_err(&engine, "SELECT * FROM sales GROUP BY region");
        assert!(matches!(err, ExecError::InvalidAggregate(_)));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn group_by_unknown_column_is_an_error() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let err = run_err(&engine, "SELECT COUNT(*) FROM sales GROUP BY nope");
        assert!(matches!(err, ExecError::ColumnNotFound(_, _)));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn order_by_aggregate_without_grouping_is_an_error() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let err = run_err(&engine, "SELECT * FROM sales ORDER BY COUNT(*) DESC");
        assert!(matches!(err, ExecError::InvalidAggregate(_)));
        let _ = std::fs::remove_file(p);
    }

    fn index_entry_count(engine: &Engine, table: &str, column: &str) -> usize {
        engine
            .storage
            .scan_prefix(index_prefix(table, column).as_bytes())
            .unwrap()
            .len()
    }

    /// Run the same SELECT on a table with and without an index and demand identical output.
    fn assert_same_with_and_without_index(setup: &[&str], index_sql: &str, queries: &[&str]) {
        let (plain, p1) = temp_engine();
        let (indexed, p2) = temp_engine();
        for sql in setup {
            run(&plain, sql);
            run(&indexed, sql);
        }
        run(&indexed, index_sql);
        for q in queries {
            let a = run(&plain, q);
            let b = run(&indexed, q);
            assert_eq!(a.rows, b.rows, "query differs with index: {q}");
        }
        let _ = std::fs::remove_file(p1);
        let _ = std::fs::remove_file(p2);
    }

    #[test]
    fn create_index_reports_and_validates() {
        let (engine, p) = temp_engine();
        people(&engine);
        let result = run(&engine, "CREATE INDEX idx_age ON users (age)");
        assert!(result.message.unwrap().contains("4 row(s) indexed"));
        assert!(matches!(
            run_err(&engine, "CREATE INDEX idx_age ON users (name)"),
            ExecError::IndexExists(_)
        ));
        assert!(matches!(
            run_err(&engine, "CREATE INDEX idx_other ON users (age)"),
            ExecError::ColumnAlreadyIndexed { .. }
        ));
        assert!(matches!(
            run_err(&engine, "CREATE INDEX i2 ON users (nope)"),
            ExecError::ColumnNotFound(_, _)
        ));
        assert!(matches!(
            run_err(&engine, "CREATE INDEX i3 ON ghosts (id)"),
            ExecError::TableNotFound(_)
        ));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn index_name_is_unique_across_tables() {
        let (engine, p) = temp_engine();
        people(&engine);
        run(&engine, "CREATE TABLE t (a INTEGER)");
        run(&engine, "CREATE INDEX shared ON users (age)");
        assert!(matches!(
            run_err(&engine, "CREATE INDEX shared ON t (a)"),
            ExecError::IndexExists(_)
        ));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn indexed_integer_queries_match_full_scan() {
        // negative numbers, duplicates and NULLs exercise the order-preserving encoding
        let setup = [
            "CREATE TABLE t (id INTEGER, n INTEGER)",
            "INSERT INTO t (id, n) VALUES (1, 5), (2, -3), (3, 5), (4, 0), (5, NULL), (6, 100), (7, -100), (8, 7)",
        ];
        let queries = [
            "SELECT * FROM t WHERE n = 5",
            "SELECT * FROM t WHERE n = -3",
            "SELECT * FROM t WHERE n > 0",
            "SELECT * FROM t WHERE n >= 0",
            "SELECT * FROM t WHERE n < 0",
            "SELECT * FROM t WHERE n <= 0",
            "SELECT * FROM t WHERE n > 5 AND id > 1",
            "SELECT * FROM t WHERE n = 5 OR n = 100",
            "SELECT * FROM t WHERE n != 5",
            "SELECT * FROM t WHERE n > 1000",
            "SELECT * FROM t WHERE n < -1000",
            "SELECT * FROM t WHERE n = 'five'",
        ];
        assert_same_with_and_without_index(&setup, "CREATE INDEX idx_n ON t (n)", &queries);
    }

    #[test]
    fn indexed_text_queries_match_full_scan() {
        let setup = [
            "CREATE TABLE t (id INTEGER, s TEXT)",
            "INSERT INTO t (id, s) VALUES (1, 'a'), (2, 'ab'), (3, 'abc'), (4, 'b'), (5, ''), (6, 'a:b'), (7, 'B'), (8, NULL), (9, 'ab')",
        ];
        let queries = [
            "SELECT * FROM t WHERE s = 'ab'",
            "SELECT * FROM t WHERE s = ''",
            "SELECT * FROM t WHERE s > 'a'",
            "SELECT * FROM t WHERE s >= 'ab'",
            "SELECT * FROM t WHERE s < 'b'",
            "SELECT * FROM t WHERE s <= 'a:b'",
            "SELECT * FROM t WHERE s > 'zzz'",
            "SELECT * FROM t WHERE s = 5",
        ];
        assert_same_with_and_without_index(&setup, "CREATE INDEX idx_s ON t (s)", &queries);
    }

    #[test]
    fn index_is_backfilled_and_maintained_on_insert_update_delete() {
        let (engine, p) = temp_engine();
        people(&engine);
        run(&engine, "CREATE INDEX idx_age ON users (age)");
        assert_eq!(index_entry_count(&engine, "users", "age"), 4);

        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (5, 'Eve', 30)",
        );
        assert_eq!(index_entry_count(&engine, "users", "age"), 5);
        let r = run(
            &engine,
            "SELECT name FROM users WHERE age = 30 ORDER BY name ASC",
        );
        assert_eq!(col(&r, 0), vec!["Alice", "Eve"]);

        run(&engine, "UPDATE users SET age = 31 WHERE name = 'Alice'");
        assert_eq!(index_entry_count(&engine, "users", "age"), 5);
        assert_eq!(
            run(&engine, "SELECT * FROM users WHERE age = 30")
                .rows
                .len(),
            1
        );
        assert_eq!(
            run(&engine, "SELECT * FROM users WHERE age = 31")
                .rows
                .len(),
            1
        );

        run(&engine, "DELETE FROM users WHERE age = 31");
        assert_eq!(index_entry_count(&engine, "users", "age"), 4);
        assert!(run(&engine, "SELECT * FROM users WHERE age = 31")
            .rows
            .is_empty());

        run(&engine, "DELETE FROM users");
        assert_eq!(index_entry_count(&engine, "users", "age"), 0);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn update_through_index_changes_only_matching_rows() {
        let (engine, p) = temp_engine();
        people(&engine);
        run(&engine, "CREATE INDEX idx_age ON users (age)");
        let r = run(&engine, "UPDATE users SET name = 'X' WHERE age >= 25");
        assert_eq!(r.message.unwrap(), "2 row(s) updated.");
        let r = run(&engine, "SELECT name FROM users ORDER BY id ASC");
        assert_eq!(col(&r, 0), vec!["X", "Bob", "X", "Dave"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn index_survives_reopen() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("snaildb_idx_reopen_{}.redb", n));
        {
            let e = Engine::open(path.to_str().unwrap()).unwrap();
            run(&e, "CREATE TABLE t (a INTEGER)");
            run(&e, "INSERT INTO t (a) VALUES (1), (2)");
            run(&e, "CREATE INDEX ia ON t (a)");
        }
        let e = Engine::open(path.to_str().unwrap()).unwrap();
        run(&e, "INSERT INTO t (a) VALUES (3)");
        assert_eq!(index_entry_count(&e, "t", "a"), 3);
        assert_eq!(run(&e, "SELECT * FROM t WHERE a >= 2").rows.len(), 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn indexes_are_ignored_for_joins_but_results_stay_correct() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(
            &engine,
            "CREATE TABLE orders (id INTEGER, user_id INTEGER, amount INTEGER)",
        );
        run(&engine, "CREATE INDEX idx_amount ON orders (amount)");
        run(
            &engine,
            "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob')",
        );
        run(
            &engine,
            "INSERT INTO orders (id, user_id, amount) VALUES (1, 1, 100), (2, 1, 200), (3, 2, 50)",
        );
        let r = run(
            &engine,
            "SELECT users.name FROM users INNER JOIN orders ON users.id = orders.user_id WHERE amount > 60",
        );
        assert_eq!(r.rows.len(), 2);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn choose_index_prefers_equality_and_skips_unusable_conditions() {
        let mut schema = Schema::new(
            "t",
            vec![
                Column {
                    name: "a".into(),
                    col_type: ColType::Integer,
                },
                Column {
                    name: "b".into(),
                    col_type: ColType::Integer,
                },
            ],
        );
        schema.indexes.push(IndexDef {
            name: "ia".into(),
            column: "a".into(),
        });
        schema.indexes.push(IndexDef {
            name: "ib".into(),
            column: "b".into(),
        });

        let where_of = |sql: &str| -> Expr {
            match Parser::new(Lexer::new(sql).tokenize().unwrap())
                .parse()
                .unwrap()
            {
                Stmt::Select(s) => s.where_clause.unwrap(),
                _ => unreachable!(),
            }
        };

        // equality beats an earlier range condition
        let pick =
            choose_index(&schema, &where_of("SELECT * FROM t WHERE a > 1 AND b = 2")).unwrap();
        assert_eq!((pick.index.name.as_str(), pick.op), ("ib", CmpOp::Eq));
        // range is used when it is all there is
        let pick = choose_index(&schema, &where_of("SELECT * FROM t WHERE a < 9")).unwrap();
        assert_eq!(pick.index.name, "ia");
        // not indexable: !=, NULL, OR, non-indexed column
        assert!(choose_index(&schema, &where_of("SELECT * FROM t WHERE a != 1")).is_none());
        assert!(choose_index(&schema, &where_of("SELECT * FROM t WHERE a = NULL")).is_none());
        assert!(choose_index(&schema, &where_of("SELECT * FROM t WHERE a = 1 OR b = 2")).is_none());
        assert!(choose_index(&schema, &where_of("SELECT * FROM t WHERE c = 1")).is_none());
    }

    #[test]
    fn rollback_undoes_inserts_updates_deletes_and_index_entries() {
        let (engine, p) = temp_engine();
        people(&engine);
        run(&engine, "CREATE INDEX idx_age ON users (age)");
        run(&engine, "BEGIN");
        run(
            &engine,
            "INSERT INTO users (id, name, age) VALUES (9, 'Zed', 99)",
        );
        run(&engine, "UPDATE users SET age = 1 WHERE id = 1");
        run(&engine, "DELETE FROM users WHERE id = 2");
        // reads inside the transaction see the uncommitted state, through the index too
        assert_eq!(
            run(&engine, "SELECT * FROM users WHERE age = 99")
                .rows
                .len(),
            1
        );
        assert_eq!(
            run(&engine, "SELECT COUNT(*) FROM users").rows[0][0],
            Some("4".to_string())
        );
        run(&engine, "ROLLBACK");
        let r = run(&engine, "SELECT id, age FROM users ORDER BY id ASC");
        assert_eq!(col(&r, 1), vec!["30", "20", "25", "NULL"]);
        assert!(run(&engine, "SELECT * FROM users WHERE age = 99")
            .rows
            .is_empty());
        assert_eq!(index_entry_count(&engine, "users", "age"), 4);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn commit_makes_changes_durable() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("snaildb_commit_{}.redb", n));
        {
            let e = Engine::open(path.to_str().unwrap()).unwrap();
            run(&e, "CREATE TABLE t (a INTEGER)");
            run(&e, "BEGIN");
            run(&e, "INSERT INTO t (a) VALUES (1), (2)");
            run(&e, "COMMIT");
            run(&e, "BEGIN");
            run(&e, "INSERT INTO t (a) VALUES (3)");
            // dropped without COMMIT
        }
        let e = Engine::open(path.to_str().unwrap()).unwrap();
        assert_eq!(run(&e, "SELECT * FROM t").rows.len(), 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ddl_is_transactional() {
        let (engine, p) = temp_engine();
        run(&engine, "BEGIN");
        run(&engine, "CREATE TABLE t (a INTEGER)");
        run(&engine, "INSERT INTO t (a) VALUES (1)");
        run(&engine, "ROLLBACK");
        assert!(matches!(
            run_err(&engine, "SELECT * FROM t"),
            ExecError::TableNotFound(_)
        ));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn transaction_state_errors() {
        let (engine, p) = temp_engine();
        assert!(matches!(
            run_err(&engine, "COMMIT"),
            ExecError::NoTransaction
        ));
        assert!(matches!(
            run_err(&engine, "ROLLBACK"),
            ExecError::NoTransaction
        ));
        run(&engine, "BEGIN");
        assert!(engine.in_transaction());
        assert!(matches!(
            run_err(&engine, "BEGIN"),
            ExecError::TransactionActive
        ));
        run(&engine, "COMMIT");
        assert!(!engine.in_transaction());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn failed_multi_row_insert_leaves_nothing_behind() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE t (id INTEGER)");
        run(&engine, "CREATE INDEX idx_id ON t (id)");
        // the second row cannot be coerced to INTEGER
        let err = run_err(&engine, "INSERT INTO t (id) VALUES (1), ('oops')");
        assert!(matches!(err, ExecError::TypeMismatch { .. }));
        assert!(run(&engine, "SELECT * FROM t").rows.is_empty());
        assert_eq!(index_entry_count(&engine, "t", "id"), 0);
        assert!(!engine.in_transaction());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn failed_statement_inside_transaction_keeps_the_transaction_open() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE t (id INTEGER)");
        run(&engine, "BEGIN");
        run(&engine, "INSERT INTO t (id) VALUES (1)");
        let _ = run_err(&engine, "INSERT INTO t (id) VALUES (2), ('bad')");
        assert!(engine.in_transaction());
        run(&engine, "COMMIT");
        // only the first, successful statement survived
        assert_eq!(col(&run(&engine, "SELECT id FROM t"), 0), vec!["1"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn row_ids_roll_back_too() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE t (id INTEGER)");
        run(&engine, "BEGIN");
        run(&engine, "INSERT INTO t (id) VALUES (1), (2), (3)");
        run(&engine, "ROLLBACK");
        run(&engine, "INSERT INTO t (id) VALUES (7)");
        let keys = engine.storage.scan_prefix(b"row:t:").unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(row_id_from_key(&keys[0].0), 1, "ids restart after rollback");
        let _ = std::fs::remove_file(p);
    }

    fn plan(engine: &Engine, sql: &str) -> Vec<String> {
        let r = run(engine, &format!("EXPLAIN {}", sql));
        assert_eq!(r.columns, vec!["plan"]);
        col(&r, 0)
    }

    #[test]
    fn explain_shows_full_scan_then_index_scan() {
        let (engine, p) = temp_engine();
        people(&engine);
        assert_eq!(
            plan(&engine, "SELECT * FROM users WHERE age > 25")[0],
            "FULL SCAN users"
        );
        run(&engine, "CREATE INDEX idx_age ON users (age)");
        let lines = plan(&engine, "SELECT * FROM users WHERE age > 25");
        assert_eq!(lines[0], "INDEX SCAN users using idx_age (age > 25)");
        assert_eq!(lines[1], "FILTER age > 25");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn explain_does_not_run_the_statement() {
        let (engine, p) = temp_engine();
        people(&engine);
        plan(&engine, "DELETE FROM users");
        plan(&engine, "UPDATE users SET age = 1");
        assert_eq!(run(&engine, "SELECT * FROM users").rows.len(), 4);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn explain_index_choice_rules() {
        let (engine, p) = temp_engine();
        people(&engine);
        run(&engine, "CREATE INDEX idx_age ON users (age)");
        // OR, != and non-indexed columns fall back to a full scan
        assert_eq!(
            plan(&engine, "SELECT * FROM users WHERE age = 1 OR age = 2")[0],
            "FULL SCAN users"
        );
        assert_eq!(
            plan(&engine, "SELECT * FROM users WHERE age != 1")[0],
            "FULL SCAN users"
        );
        assert_eq!(
            plan(&engine, "SELECT * FROM users WHERE name = 'Bob'")[0],
            "FULL SCAN users"
        );
        // an indexed condition inside an AND is found
        assert_eq!(
            plan(
                &engine,
                "SELECT * FROM users WHERE name = 'Bob' AND age = 20"
            )[0],
            "INDEX SCAN users using idx_age (age = 20)"
        );
        // UPDATE and DELETE use indexes too
        let lines = plan(&engine, "DELETE FROM users WHERE age = 20");
        assert_eq!(lines[0], "INDEX SCAN users using idx_age (age = 20)");
        assert_eq!(lines[2], "DELETE matching rows");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn explain_describes_every_select_stage() {
        let (engine, p) = temp_engine();
        sales(&engine);
        let lines = plan(
            &engine,
            "SELECT region, SUM(amount) FROM sales WHERE amount > 1 GROUP BY region \
             HAVING COUNT(*) > 1 OR SUM(amount) < 5 ORDER BY SUM(amount) DESC LIMIT 3",
        );
        assert_eq!(
            lines,
            vec![
                "FULL SCAN sales",
                "FILTER amount > 1",
                "GROUP BY region",
                "HAVING COUNT(*) > 1 OR SUM(amount) < 5",
                "SORT BY SUM(amount) DESC",
                "LIMIT 3",
            ]
        );
        let lines = plan(&engine, "SELECT COUNT(*) FROM sales");
        assert_eq!(lines[1], "AGGREGATE (single group)");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn explain_join_and_errors() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE users (id INTEGER, name TEXT)");
        run(&engine, "CREATE TABLE orders (id INTEGER, user_id INTEGER)");
        let lines = plan(
            &engine,
            "SELECT users.name FROM users LEFT JOIN orders ON users.id = orders.user_id",
        );
        assert_eq!(
            lines[1],
            "LEFT JOIN orders ON users.id = orders.user_id (nested loop)"
        );
        assert!(matches!(
            run_err(&engine, "EXPLAIN SELECT * FROM ghosts"),
            ExecError::TableNotFound(_)
        ));
        assert!(matches!(
            run_err(
                &engine,
                "EXPLAIN SELECT * FROM users JOIN ghosts ON users.id = ghosts.id"
            ),
            ExecError::TableNotFound(_)
        ));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn show_tables_and_describe() {
        let (engine, p) = temp_engine();
        assert!(run(&engine, "SHOW TABLES").rows.is_empty());
        people(&engine);
        run(&engine, "CREATE TABLE a_first (x INTEGER)");
        run(&engine, "CREATE INDEX idx_age ON users (age)");
        assert_eq!(
            col(&run(&engine, "SHOW TABLES"), 0),
            vec!["a_first", "users"]
        );

        let d = run(&engine, "DESCRIBE users");
        assert_eq!(d.columns, vec!["column", "type", "index"]);
        assert_eq!(col(&d, 0), vec!["id", "name", "age"]);
        assert_eq!(col(&d, 1), vec!["INTEGER", "TEXT", "INTEGER"]);
        assert_eq!(col(&d, 2), vec!["", "", "idx_age"]);
        assert!(matches!(
            run_err(&engine, "DESCRIBE ghosts"),
            ExecError::TableNotFound(_)
        ));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn where_precedence_end_to_end() {
        let (engine, p) = temp_engine();
        people(&engine);
        // Carol is not in `people`; use Alice(30), Bob(20), Carol(25) from a fresh table
        run(&engine, "CREATE TABLE p (name TEXT, age INTEGER)");
        run(
            &engine,
            "INSERT INTO p (name, age) VALUES ('Alice', 30), ('Bob', 20), ('Carol', 25)",
        );
        // AND binds tighter: Carol OR (age 20 AND Alice) -> only Carol
        let r = run(
            &engine,
            "SELECT name FROM p WHERE name = 'Carol' OR age = 20 AND name = 'Alice'",
        );
        assert_eq!(col(&r, 0), vec!["Carol"]);
        // parentheses change the answer: (Carol OR age 20) AND Alice -> nobody
        let r = run(
            &engine,
            "SELECT name FROM p WHERE (name = 'Carol' OR age = 20) AND name = 'Alice'",
        );
        assert!(r.rows.is_empty());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn quotes_round_trip_through_insert_and_where() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE t (s TEXT)");
        run(&engine, "INSERT INTO t (s) VALUES ('it''s'), ('plain')");
        let r = run(&engine, "SELECT s FROM t WHERE s = 'it''s'");
        assert_eq!(col(&r, 0), vec!["it's"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn corrupt_row_is_an_error_not_a_panic() {
        let (engine, p) = temp_engine();
        run(&engine, "CREATE TABLE t (a INTEGER)");
        engine
            .storage
            .put(b"row:t:00000000000000000001", b"not json")
            .unwrap();
        assert!(matches!(
            run_err(&engine, "SELECT * FROM t"),
            ExecError::Json(_)
        ));
        let _ = std::fs::remove_file(p);
    }
}
