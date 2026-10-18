# Changelog

All notable changes to SnailDB are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.11.0]

### Added
- Transactions: `BEGIN`, `COMMIT`, `ROLLBACK`. Writes are buffered inside `Storage`; reads inside the transaction see them (including through indexes); `COMMIT` applies everything in a single atomic redb transaction; `ROLLBACK` discards the buffer. DDL (`CREATE TABLE`, `CREATE INDEX`) is transactional too.
- Every write statement is now atomic. A multi-row `INSERT` that fails halfway no longer leaves the earlier rows behind. Inside an explicit transaction a failed statement is undone but the transaction stays open.
- `BEGIN` inside a transaction, and `COMMIT`/`ROLLBACK` outside one, are clear errors.
- The semantic cache bypasses caching while a transaction is open and clears itself on `BEGIN`, `COMMIT` and `ROLLBACK`.

### Changed
- A write statement is now one durable commit instead of one per row, so bulk inserts are much faster.

### Notes
- Closing the database with a transaction still open discards it (equivalent to `ROLLBACK`).
- Isolation: SnailDB is single-connection; there is no concurrent-transaction handling.

## [0.10.0]

### Added
- `CREATE INDEX name ON table (column)` on INTEGER or TEXT columns. Existing rows are backfilled.
- Index-aware `SELECT`, `UPDATE` and `DELETE`: equality (`=`) and range (`<`, `<=`, `>`, `>=`) conditions on an indexed column narrow the scan. An index inside an `AND` is used; under an `OR`, with `!=`, or with `NULL` it is not.
- Indexes are kept in sync on `INSERT`, `UPDATE` and `DELETE`, and persist across restarts.
- `Storage::scan_range` for half-open key-range scans.
- Benchmark for full scan vs. indexed point lookup.

### Notes
- The index only narrows candidates; the full `WHERE` is still evaluated, so results are identical with or without an index (covered by tests that run each query both ways). Index-narrowed results come back in row-id order, same as a full scan.
- Queries with `JOIN` use a full scan.
- Schemas written by earlier versions still load (the new `indexes` field defaults to empty).

## [0.9.0]

### Added
- `GROUP BY` (one or more columns, including qualified names and joins).
- `HAVING` with `AND` / `OR` over aggregate comparisons, e.g. `HAVING COUNT(*) > 1`. Standard precedence: `AND` binds tighter than `OR`.
- `ORDER BY` an aggregate on grouped queries: `ORDER BY SUM(amount) DESC`.
- `GROUP` and `HAVING` keywords.
- `ASK` handles "per" / "for each" questions ("how many users per age").

### Changed
- Aggregate results are typed internally, so `HAVING` and `ORDER BY` compare numbers numerically (`35 > 9`, not `"35" < "9"`).
- Groups appear in first-seen order; `NULL` values form their own group; `GROUP BY` over an empty table returns no rows, while a plain aggregate over an empty table still returns one row.
- Selecting a column that is not in `GROUP BY`, or using `SELECT *` with `GROUP BY`, is an error.

## [0.8.0]

### Added
- Aggregate functions `COUNT(*)`, `COUNT(col)`, `SUM`, `AVG`, `MIN`, `MAX`, usable with `WHERE` and over joins.
- SQL semantics: NULLs are ignored by every aggregate except `COUNT(*)`; aggregates over an empty set return `NULL` (`COUNT` returns `0`); `SUM`/`AVG` on a TEXT column is a type error.
- `ASK` understands aggregate questions ("how many users", "average age of users").
- Joining a table that does not exist is now a clear `TableNotFound` error instead of silently returning no rows.

### Changed
- Aggregate names are not keywords, so columns named `count` or `max` keep working.
- Refactored duplicated column lookup into a shared `lookup_column` helper.

## [0.7.0]

### Added
- Python test suite (`tests/`): ASK translator, semantic cache and end-to-end engine tests through the PyO3 bridge.
- CI job that builds the extension with maturin and runs pytest.
- `benchmarks/bench.py` comparing a raw engine query with a semantic cache hit.
- Exact-match fast path in the cache (normalized SQL: case, whitespace and `;` ignored).
- `SemanticCache(embedder=..., threshold=..., max_entries=...)`: injectable embedder, configurable threshold, bounded size.

### Fixed
- **Cache returned wrong rows for near-identical queries.** `WHERE id = 1` and `WHERE id = 2` embed almost identically, so a semantic hit could return the other query's result. A hit now also requires the same literals, same remaining tokens and same SELECT list.
- `SemanticCache._is_write` raised `IndexError` on empty input.
- `sentence-transformers` is now imported lazily, so tests and tools do not need the model.

## [0.6.0]

### Added
- GitHub Actions CI: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`.
- `LICENSE` (MIT), `CONTRIBUTING.md`, `CHANGELOG.md`.
- `docs/ARCHITECTURE.md` describing the layers, key layout and query pipeline.

### Changed
- Applied `rustfmt` and fixed all clippy warnings in `snail_core`.
- Fixed `.gitignore` (the `__pycache__` rule was merged into the previous line and never worked).

## [0.5.0]
- Q-learning query optimizer with a persisted Q-table.

## [0.4.0]
- Natural-language `ASK` command.

## [0.3.0]
- `INNER JOIN` and `LEFT JOIN`.

## [0.2.0]
- `UPDATE`, `DELETE`, `ORDER BY`, `LIMIT`.

## [0.1.0]
- redb storage, catalog, SQL lexer/parser, `CREATE`/`INSERT`/`SELECT`/`WHERE`, semantic cache, REPL.
