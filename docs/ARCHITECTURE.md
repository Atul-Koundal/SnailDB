# SnailDB Architecture

SnailDB has three layers. Each layer only talks to the one directly below it.

```
snail_cli   (Python REPL)
    |
snail_ai    (Python: semantic cache, ASK translator, RL optimizer)
    |
snail_core  (Rust: storage, catalog, SQL parser, executor)  <- exposed via PyO3
```

## snail_core

| Module | Responsibility |
|---|---|
| `storage` | Thin wrapper over [redb](https://github.com/cberner/redb): `get`, `put`, `delete`, `scan_prefix`, `delete_prefix`. Every other module goes through this. |
| `catalog` | Persists table schemas (`ColType`, `Column`, `Schema`). |
| `sql::lexer` | Turns a SQL string into `Token`s. |
| `sql::parser` | Hand-written recursive descent parser producing an AST (`sql::ast`). |
| `executor` | Walks the AST against storage and returns a `QueryResult`. |
| `lib.rs` | PyO3 bridge: exposes a `SnailDB` class with `execute(sql) -> dict`. |

### Key layout in redb

Everything lives in a single redb table of raw bytes. Namespaces are separated by key prefix:

| Key | Value |
|---|---|
| `catalog:<table>` | JSON-serialised `Schema` |
| `row:<table>:<20-digit id>` | JSON object of the row |
| `meta:<table>:next_id` | next auto-increment row id, as text |

Zero-padded row ids keep rows in insertion order during a prefix scan.

### Query pipeline

```
SQL string -> Lexer -> Vec<Token> -> Parser -> Stmt (AST) -> Engine::execute -> QueryResult
```

For `SELECT`: scan base table -> apply joins -> `WHERE` -> `ORDER BY` -> `LIMIT` -> project columns.

Rows are held as flat maps of `"table.column" -> JSON value`, which lets joins and qualified or unqualified column references share one code path.

## snail_ai

- **Semantic cache**: embeds each `SELECT` with `all-MiniLM-L6-v2`; a cosine similarity of 0.92 or higher returns the cached result. Any write clears the cache.
- **ASK translator**: rule-based natural language to SQL, with a word-overlap fallback over past questions.
- **RL optimizer**: Q-learning agent over query types, with a Q-table persisted to disk.

## snail_cli

An interactive REPL that wires the layers together and pretty-prints results.
