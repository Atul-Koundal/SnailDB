# Changelog

All notable changes to SnailDB are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

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
