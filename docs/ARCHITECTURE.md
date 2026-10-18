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
| `idx:<table>:<column>:<enc>!<row id>` | empty; one entry per row per indexed column |

Zero-padded row ids keep rows in insertion order during a prefix scan.

### Indexes

An index entry's key embeds an order-preserving encoding of the value: integers are `i` plus 16 hex digits of the sign-flipped value, text is `t` plus the hex of its bytes, NULL is `n`. The separator `!` sorts below every hex digit, so entries sort by value and then by row id. That makes `=` a prefix scan and `<`, `<=`, `>`, `>=` a range scan over one type tag.

The planner (`choose_index`) looks only at AND-ed conditions, prefers equality, and ignores `!=`, NULL comparisons and anything under an OR. An index only narrows the candidate rows; the full WHERE is still evaluated, and candidates are sorted by row id, so results never depend on whether an index was used.

### Transactions

`Storage` keeps an optional in-memory write buffer (`key -> Some(value) | None`). While a transaction is open, `put` and `delete` go to the buffer, and `get`, `scan_prefix` and `scan_range` read the database with the buffer layered on top. `COMMIT` writes the whole buffer in one redb write transaction (atomic and durable); `ROLLBACK` drops it.

The engine also wraps every write statement in an implicit transaction and takes a snapshot of the buffer first. If the statement fails, the snapshot is restored, so a statement is all-or-nothing, and inside an explicit transaction a failed statement does not abort the rest of it.

### Introspection

`EXPLAIN` reuses the same planner (`choose_index`) as execution, so it cannot drift from what actually runs. `SHOW TABLES` and `DESCRIBE` read the catalog; the REPL uses them to discover tables and to give the LLM its schema context.

### Query pipeline

```
SQL string -> Lexer -> Vec<Token> -> Parser -> Stmt (AST) -> Engine::execute -> QueryResult
```

For `SELECT`: scan base table -> apply joins -> `WHERE` -> `ORDER BY` -> `LIMIT` -> project columns.

For aggregate or grouped selects the pipeline is: scan base table -> joins -> `WHERE` -> group rows by the `GROUP BY` key (one group if there is none) -> compute aggregates per group -> `HAVING` -> `ORDER BY` -> `LIMIT`. Computed values are kept typed (`Cell`) until the final render step so `HAVING` and `ORDER BY` compare numbers numerically. Aggregate names are parsed as functions only when followed by `(`, so they are not reserved words.

Rows are held as flat maps of `"table.column" -> JSON value`, which lets joins and qualified or unqualified column references share one code path.

## snail_ai

- **Semantic cache**: a `SELECT` is looked up in three steps: exact match on the normalized SQL, then embedding similarity (`all-MiniLM-L6-v2`, threshold 0.92) gated by a *signature guard*, then the engine. The guard requires the same ordered literals, the same bag of other tokens and the same SELECT list, because embeddings cannot tell `id = 1` from `id = 2`. Writes clear the cache.
- **ASK translator**: rule-based natural language to SQL, with a word-overlap fallback over past questions.
- **LLM translator** (optional): when the rules fail, `ASK` sends the question and your table schemas to a local Ollama model. Only a single `SELECT` is accepted from it, and the REPL validates it with `EXPLAIN` before running it.
- **RL optimizer**: Q-learning agent over query types, with a Q-table persisted to disk.

## snail_cli

An interactive REPL that wires the layers together and pretty-prints results.
