use crate::storage::{Storage, StorageError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Column types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ColType {
    Integer,
    Text,
}

impl ColType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "INTEGER" | "INT" => Some(ColType::Integer),
            "TEXT"    | "VARCHAR" => Some(ColType::Text),
            _ => None,
        }
    }
}

// ── Schema ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub col_type: ColType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    pub table_name: String,
    pub columns: Vec<Column>,
}

impl Schema {
    pub fn new(table_name: &str, columns: Vec<Column>) -> Self {
        Schema {
            table_name: table_name.to_string(),
            columns,
        }
    }

    /// Returns the index of a column by name, or None.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("table '{0}' already exists")]
    TableAlreadyExists(String),

    #[error("table '{0}' does not exist")]
    TableNotFound(String),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("schema deserialise error: {0}")]
    Deserialise(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, CatalogError>;

// ── Catalog ───────────────────────────────────────────────────────────────────

/// Key prefix used for all catalog entries in redb.
fn catalog_key(table_name: &str) -> Vec<u8> {
    format!("catalog:{}", table_name).into_bytes()
}

pub struct Catalog<'a> {
    storage: &'a Storage,
}

impl<'a> Catalog<'a> {
    pub fn new(storage: &'a Storage) -> Self {
        Catalog { storage }
    }

    /// Persist a new schema. Fails if the table already exists.
    pub fn create_table(&self, schema: Schema) -> Result<()> {
        let key = catalog_key(&schema.table_name);
        if self.storage.get(&key)?.is_some() {
            return Err(CatalogError::TableAlreadyExists(schema.table_name));
        }
        let bytes = serde_json::to_vec(&schema)?;
        self.storage.put(&key, &bytes)?;
        Ok(())
    }

    /// Load a schema by table name.
    pub fn get_schema(&self, table_name: &str) -> Result<Schema> {
        let key = catalog_key(table_name);
        match self.storage.get(&key)? {
            Some(bytes) => Ok(serde_json::from_slice(&bytes)?),
            None => Err(CatalogError::TableNotFound(table_name.to_string())),
        }
    }

    /// Returns true if the table exists.
    pub fn table_exists(&self, table_name: &str) -> Result<bool> {
        let key = catalog_key(table_name);
        Ok(self.storage.get(&key)?.is_some())
    }

    /// Drop a table's schema. Does NOT remove row data — the executor
    /// handles that via storage.delete_prefix("row:<table>:").
    pub fn drop_table(&self, table_name: &str) -> Result<()> {
        let key = catalog_key(table_name);
        let existed = self.storage.delete(&key)?;
        if !existed {
            return Err(CatalogError::TableNotFound(table_name.to_string()));
        }
        Ok(())
    }

    /// List all table names currently in the catalog.
    pub fn list_tables(&self) -> Result<Vec<String>> {
        let prefix = b"catalog:";
        let pairs = self.storage.scan_prefix(prefix)?;
        let names = pairs
            .into_iter()
            .map(|(k, _)| {
                // Strip the "catalog:" prefix to get the bare table name
                let s = String::from_utf8_lossy(&k);
                s["catalog:".len()..].to_string()
            })
            .collect();
        Ok(names)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_storage() -> (Storage, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("snaildb_catalog_test_{}.redb", n));
        let storage = Storage::open(&path).unwrap();
        (storage, path)
    }

    fn make_schema(name: &str) -> Schema {
        Schema::new(name, vec![
            Column { name: "id".to_string(),   col_type: ColType::Integer },
            Column { name: "name".to_string(), col_type: ColType::Text    },
        ])
    }

    #[test]
    fn create_and_get_schema() {
        let (storage, p) = temp_storage();
        let catalog = Catalog::new(&storage);

        catalog.create_table(make_schema("users")).unwrap();
        let schema = catalog.get_schema("users").unwrap();
        assert_eq!(schema.table_name, "users");
        assert_eq!(schema.columns.len(), 2);
        assert_eq!(schema.columns[0].name, "id");
        assert_eq!(schema.columns[1].col_type, ColType::Text);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn duplicate_table_is_error() {
        let (storage, p) = temp_storage();
        let catalog = Catalog::new(&storage);

        catalog.create_table(make_schema("users")).unwrap();
        let err = catalog.create_table(make_schema("users")).unwrap_err();
        assert!(matches!(err, CatalogError::TableAlreadyExists(_)));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn get_missing_table_is_error() {
        let (storage, p) = temp_storage();
        let catalog = Catalog::new(&storage);

        let err = catalog.get_schema("ghost").unwrap_err();
        assert!(matches!(err, CatalogError::TableNotFound(_)));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn drop_table() {
        let (storage, p) = temp_storage();
        let catalog = Catalog::new(&storage);

        catalog.create_table(make_schema("users")).unwrap();
        assert!(catalog.table_exists("users").unwrap());

        catalog.drop_table("users").unwrap();
        assert!(!catalog.table_exists("users").unwrap());

        // dropping again errors
        let err = catalog.drop_table("users").unwrap_err();
        assert!(matches!(err, CatalogError::TableNotFound(_)));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn list_tables() {
        let (storage, p) = temp_storage();
        let catalog = Catalog::new(&storage);

        catalog.create_table(make_schema("users")).unwrap();
        catalog.create_table(make_schema("products")).unwrap();

        let mut tables = catalog.list_tables().unwrap();
        tables.sort();
        assert_eq!(tables, vec!["products", "users"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn column_index() {
        let schema = make_schema("users");
        assert_eq!(schema.column_index("id"),   Some(0));
        assert_eq!(schema.column_index("name"), Some(1));
        assert_eq!(schema.column_index("age"),  None);
    }
}