# Contributing to SnailDB

Thanks for your interest. SnailDB is a learning-driven project, so small, well-explained changes are preferred over large ones.

## Setup

```bash
# Rust core (stable toolchain)
cd snail_core
cargo test

# Python extension + AI layer
pip install -r snail_cli/requirements.txt
maturin develop
```

## Before you open a pull request

Run the same checks CI runs:

```bash
cd snail_core
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Guidelines

- One feature per pull request, with tests.
- New SQL features need a lexer/parser test **and** an executor test.
- Keep the layering: `snail_cli` talks to `snail_ai`, `snail_ai` talks to `snail_core`. Nothing reaches upward.
- Update `CHANGELOG.md` and the README feature list when you add a feature.
