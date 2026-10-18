# Changelog

All notable changes to SnailDB are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

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
