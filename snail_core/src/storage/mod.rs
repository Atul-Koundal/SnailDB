use redb::{Database, TableDefinition};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
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

/// Pending writes of an open transaction: `Some(value)` = put, `None` = delete.
pub type TxnBuffer = BTreeMap<Vec<u8>, Option<Vec<u8>>>;

pub struct Storage {
    db: Database,
    /// `Some` while a transaction is open. Writes go here instead of to disk; reads
    /// see the buffer layered over the database. `commit` applies it in one atomic
    /// redb write transaction, `rollback` simply drops it.
    txn: Mutex<Option<TxnBuffer>>,
}

impl Storage {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = Database::create(path)?;
        let txn = db.begin_write()?;
        {
            let _ = txn.open_table(KV_TABLE)?;
        }
        txn.commit()?;
        Ok(Storage {
            db,
            txn: Mutex::new(None),
        })
    }

    fn buffer(&self) -> MutexGuard<'_, Option<TxnBuffer>> {
        self.txn.lock().expect("transaction lock poisoned")
    }

    // ── Transactions ──────────────────────────────────────────────────

    pub fn in_transaction(&self) -> bool {
        self.buffer().is_some()
    }

    /// Start buffering writes. Returns `false` if a transaction is already open.
    pub fn begin(&self) -> bool {
        let mut buf = self.buffer();
        if buf.is_some() {
            return false;
        }
        *buf = Some(BTreeMap::new());
        true
    }

    /// Atomically write all buffered changes. Returns `false` if no transaction is open.
    pub fn commit(&self) -> Result<bool> {
        let Some(pending) = self.buffer().take() else {
            return Ok(false);
        };
        if pending.is_empty() {
            return Ok(true);
        }
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV_TABLE)?;
            for (key, value) in &pending {
                match value {
                    Some(v) => {
                        table.insert(key.as_slice(), v.as_slice())?;
                    }
                    None => {
                        table.remove(key.as_slice())?;
                    }
                }
            }
        }
        txn.commit()?;
        Ok(true)
    }

    /// Discard buffered changes. Returns `false` if no transaction is open.
    pub fn rollback(&self) -> bool {
        self.buffer().take().is_some()
    }

    /// Copy of the current buffer (`None` if no transaction), used to undo one failed
    /// statement without losing the rest of the transaction.
    pub fn snapshot(&self) -> Option<TxnBuffer> {
        self.buffer().clone()
    }

    pub fn restore(&self, snapshot: Option<TxnBuffer>) {
        *self.buffer() = snapshot;
    }

    // ── Reads and writes ──────────────────────────────────────────────

    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        if let Some(buf) = self.buffer().as_ref() {
            if let Some(entry) = buf.get(key) {
                return Ok(entry.clone());
            }
        }
        self.db_get(key)
    }

    fn db_get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV_TABLE)?;
        Ok(table.get(key)?.map(|v| v.value().to_vec()))
    }

    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        if let Some(buf) = self.buffer().as_mut() {
            buf.insert(key.to_vec(), Some(value.to_vec()));
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV_TABLE)?;
            table.insert(key, value)?;
        }
        txn.commit()?;
        Ok(())
    }

    pub fn delete(&self, key: &[u8]) -> Result<bool> {
        let buffered = {
            let mut guard = self.buffer();
            guard.as_mut().map(|buf| {
                let existed_in_buf = matches!(buf.get(key), Some(Some(_)));
                let known = buf.contains_key(key);
                buf.insert(key.to_vec(), None);
                (existed_in_buf, known)
            })
        };
        if let Some((existed_in_buf, known)) = buffered {
            // If the buffer already had an entry for this key it decides; otherwise ask the db.
            return Ok(if known {
                existed_in_buf
            } else {
                self.db_get(key)?.is_some()
            });
        }
        let txn = self.db.begin_write()?;
        let existed;
        {
            let mut table = txn.open_table(KV_TABLE)?;
            existed = table.remove(key)?.is_some();
        }
        txn.commit()?;
        Ok(existed)
    }

    pub fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV_TABLE)?;
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        for entry in table.range(prefix..)? {
            let (k, v) = entry?;
            if !k.value().starts_with(prefix) {
                break;
            }
            merged.insert(k.value().to_vec(), v.value().to_vec());
        }
        if let Some(buf) = self.buffer().as_ref() {
            for (k, v) in buf.range(prefix.to_vec()..) {
                if !k.starts_with(prefix) {
                    break;
                }
                overlay(&mut merged, k, v);
            }
        }
        Ok(merged.into_iter().collect())
    }

    /// All entries with `start <= key < end`, in key order.
    pub fn scan_range(&self, start: &[u8], end: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if start >= end {
            return Ok(Vec::new());
        }
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV_TABLE)?;
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        for entry in table.range(start..end)? {
            let (k, v) = entry?;
            merged.insert(k.value().to_vec(), v.value().to_vec());
        }
        if let Some(buf) = self.buffer().as_ref() {
            for (k, v) in buf.range(start.to_vec()..end.to_vec()) {
                overlay(&mut merged, k, v);
            }
        }
        Ok(merged.into_iter().collect())
    }

    pub fn delete_prefix(&self, prefix: &[u8]) -> Result<usize> {
        let keys: Vec<Vec<u8>> = self
            .scan_prefix(prefix)?
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        if keys.is_empty() {
            return Ok(0);
        }
        if let Some(buf) = self.buffer().as_mut() {
            for k in &keys {
                buf.insert(k.clone(), None);
            }
            return Ok(keys.len());
        }
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV_TABLE)?;
            for k in &keys {
                table.remove(k.as_slice())?;
            }
        }
        txn.commit()?;
        Ok(keys.len())
    }
}

fn overlay(merged: &mut BTreeMap<Vec<u8>, Vec<u8>>, key: &[u8], value: &Option<Vec<u8>>) {
    match value {
        Some(v) => {
            merged.insert(key.to_vec(), v.clone());
        }
        None => {
            merged.remove(key);
        }
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
    fn scan_range_is_half_open_and_ordered() {
        let (s, p) = temp_storage();
        for k in ["a", "b", "c", "d"] {
            s.put(k.as_bytes(), b"v").unwrap();
        }
        let keys: Vec<Vec<u8>> = s
            .scan_range(b"b", b"d")
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, vec![b"b".to_vec(), b"c".to_vec()]);
        assert!(s.scan_range(b"d", b"b").unwrap().is_empty());
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
        {
            let s = Storage::open(&path).unwrap();
            s.put(b"key1", b"value1").unwrap();
        }
        {
            let s = Storage::open(&path).unwrap();
            assert_eq!(s.get(b"key1").unwrap(), Some(b"value1".to_vec()));
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn transaction_buffers_until_commit() {
        let (s, p) = temp_storage();
        s.put(b"a", b"1").unwrap();
        assert!(s.begin());
        assert!(!s.begin(), "nested begin must be refused");
        s.put(b"b", b"2").unwrap();
        assert_eq!(s.get(b"b").unwrap(), Some(b"2".to_vec()));
        // not on disk yet
        assert_eq!(s.db_get(b"b").unwrap(), None);
        assert!(s.commit().unwrap());
        assert_eq!(s.db_get(b"b").unwrap(), Some(b"2".to_vec()));
        assert!(!s.in_transaction());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn rollback_discards_puts_and_deletes() {
        let (s, p) = temp_storage();
        s.put(b"keep", b"1").unwrap();
        s.begin();
        s.put(b"new", b"2").unwrap();
        assert!(s.delete(b"keep").unwrap());
        assert_eq!(s.get(b"keep").unwrap(), None);
        assert!(s.rollback());
        assert_eq!(s.get(b"keep").unwrap(), Some(b"1".to_vec()));
        assert_eq!(s.get(b"new").unwrap(), None);
        assert!(!s.rollback(), "nothing to roll back");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn scans_see_buffered_changes() {
        let (s, p) = temp_storage();
        s.put(b"k:1", b"a").unwrap();
        s.put(b"k:2", b"b").unwrap();
        s.begin();
        s.put(b"k:3", b"c").unwrap();
        s.delete(b"k:1").unwrap();
        s.put(b"k:2", b"B").unwrap();
        let rows = s.scan_prefix(b"k:").unwrap();
        assert_eq!(
            rows,
            vec![
                (b"k:2".to_vec(), b"B".to_vec()),
                (b"k:3".to_vec(), b"c".to_vec())
            ]
        );
        let ranged = s.scan_range(b"k:2", b"k:9").unwrap();
        assert_eq!(ranged.len(), 2);
        s.rollback();
        assert_eq!(s.scan_prefix(b"k:").unwrap().len(), 2);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn delete_reports_existence_inside_a_transaction() {
        let (s, p) = temp_storage();
        s.put(b"x", b"1").unwrap();
        s.begin();
        assert!(s.delete(b"x").unwrap());
        assert!(!s.delete(b"x").unwrap(), "already deleted in this txn");
        assert!(!s.delete(b"never").unwrap());
        s.rollback();
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn snapshot_and_restore_undo_part_of_a_transaction() {
        let (s, p) = temp_storage();
        s.begin();
        s.put(b"a", b"1").unwrap();
        let snap = s.snapshot();
        s.put(b"b", b"2").unwrap();
        s.restore(snap);
        assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(s.get(b"b").unwrap(), None);
        assert!(s.in_transaction());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn uncommitted_transaction_is_lost_on_reopen() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("snaildb_txn_lost_{}.redb", n));
        {
            let s = Storage::open(&path).unwrap();
            s.begin();
            s.put(b"ghost", b"1").unwrap();
        }
        let s = Storage::open(&path).unwrap();
        assert_eq!(s.get(b"ghost").unwrap(), None);
        let _ = std::fs::remove_file(path);
    }
}
