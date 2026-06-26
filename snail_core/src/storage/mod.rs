use redb::{Database, ReadableTable, TableDefinition};
use std::path::Path;
use thiserror::Error;

const KV_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("snail_kv");

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] redb::DatabaseError),
    #[error("transaction error: {0}")]
    Transaction(#[from] redb::TransactionError),
    #[error("table error: {0}")]
    Table(#[from] redb::TableError),
    #[error("storage error: {0}")]
    Storage(#[from] redb::StorageError),
    #[error("commit error: {0}")]
    Commit(#[from] redb::CommitError),
}

pub type Result<T> = std::result::Result<T, StorageError>;

pub struct Storage {
    db: Database,
}

impl Storage {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = Database::create(path)?;
        let txn = db.begin_write()?;
        { let _ = txn.open_table(KV_TABLE)?; }
        txn.commit()?;
        Ok(Storage { db })
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV_TABLE)?;
        Ok(table.get(key)?.map(|v| v.value().to_vec()))
    }

    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let txn = self.db.begin_write()?;
        { let mut table = txn.open_table(KV_TABLE)?;
          table.insert(key, value)?; }
        txn.commit()?;
        Ok(())
    }

    pub fn delete(&self, key: &[u8]) -> Result<bool> {
        let txn = self.db.begin_write()?;
        let existed;
        { let mut table = txn.open_table(KV_TABLE)?;
          existed = table.remove(key)?.is_some(); }
        txn.commit()?;
        Ok(existed)
    }

    pub fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV_TABLE)?;
        let mut results = Vec::new();
        for entry in table.range(prefix..)? {
            let (k, v) = entry?;
            if !k.value().starts_with(prefix) { break; }
            results.push((k.value().to_vec(), v.value().to_vec()));
        }
        Ok(results)
    }

    pub fn delete_prefix(&self, prefix: &[u8]) -> Result<usize> {
        let keys: Vec<Vec<u8>> = self
            .scan_prefix(prefix)?
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        if keys.is_empty() { return Ok(0); }
        let txn = self.db.begin_write()?;
        { let mut table = txn.open_table(KV_TABLE)?;
          for k in &keys { table.remove(k.as_slice())?; } }
        txn.commit()?;
        Ok(keys.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_storage() -> (Storage, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("snaildb_test_{}.redb", n));
        let storage = Storage::open(&path).unwrap();
        (storage, path)
    }

    #[test]
    fn put_get_delete() {
        let (s, p) = temp_storage();
        assert_eq!(s.get(b"foo").unwrap(), None);
        s.put(b"foo", b"bar").unwrap();
        assert_eq!(s.get(b"foo").unwrap(), Some(b"bar".to_vec()));
        s.put(b"foo", b"baz").unwrap();
        assert_eq!(s.get(b"foo").unwrap(), Some(b"baz".to_vec()));
        assert!(s.delete(b"foo").unwrap());
        assert_eq!(s.get(b"foo").unwrap(), None);
        assert!(!s.delete(b"foo").unwrap());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn scan_prefix() {
        let (s, p) = temp_storage();
        s.put(b"row:users:1", b"alice").unwrap();
        s.put(b"row:users:2", b"bob").unwrap();
        s.put(b"row:products:1", b"widget").unwrap();
        s.put(b"catalog:users", b"schema").unwrap();
        let r = s.scan_prefix(b"row:users:").unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].1, b"alice");
        assert_eq!(r[1].1, b"bob");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn delete_prefix() {
        let (s, p) = temp_storage();
        s.put(b"row:users:1", b"alice").unwrap();
        s.put(b"row:users:2", b"bob").unwrap();
        s.put(b"row:products:1", b"widget").unwrap();
        let deleted = s.delete_prefix(b"row:users:").unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(s.get(b"row:users:1").unwrap(), None);
        assert_eq!(s.get(b"row:products:1").unwrap(), Some(b"widget".to_vec()));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn persists_across_reopen() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("snaildb_persist_{}.redb", n));
        { let s = Storage::open(&path).unwrap();
          s.put(b"key1", b"value1").unwrap(); }
        { let s = Storage::open(&path).unwrap();
          assert_eq!(s.get(b"key1").unwrap(), Some(b"value1".to_vec())); }
        let _ = std::fs::remove_file(path);
    }
}