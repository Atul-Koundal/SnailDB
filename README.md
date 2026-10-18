# SnailDB

[![CI](https://github.com/Atul-Koundal/SnailDB/actions/workflows/ci.yml/badge.svg)](https://github.com/Atul-Koundal/SnailDB/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/core-Rust-orange)
![Python](https://img.shields.io/badge/AI%20layer-Python-3776AB)

An AI native SQL database built from scratch in Rust with a Python AI layer. No cloud dependencies, everything runs locally.

## What is SnailDB

SnailDB is a from scratch SQL database with an AI layer on top. The Rust core handles all storage, parsing, and execution. The Python layer adds semantic caching, natural language queries, and a reinforcement learning optimizer. The name comes from the philosophy of building slow and steady, understanding every piece before moving to the next.

## Architecture

Three layers that only talk to the layer directly below them. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full picture.

**snail_core** is the Rust engine. It handles persistent storage via redb, maintains table schemas in a catalog, parses SQL through a hand written lexer and recursive descent parser, and executes queries against real data on disk.

**snail_ai** is the Python AI layer. It has three components. The semantic cache embeds query text using all-MiniLM-L6-v2 and uses cosine similarity to return cached results for semantically similar queries. The ASK translator converts natural language questions into SQL using rule based pattern matching. The RL optimizer is a Q-learning agent that learns the best execution strategy for different query types over time and persists its knowledge to disk.

**snail_cli** is the Python REPL. It connects all three layers and gives you an interactive prompt to run SQL or ask questions in plain English.

## Project Structure

```
SnailDB/
├── snail_core/
│   ├── src/
│   │   ├── storage/     redb key value wrapper
│   │   ├── catalog/     table schema persistence
│   │   ├── sql/         lexer, parser, AST, tokens
│   │   ├── executor/    query execution engine
│   │   └── lib.rs       PyO3 bridge
│   └── Cargo.toml
├── snail_ai/
│   ├── semantic_cache.py  sentence transformer cache
│   ├── ask.py             natural language to SQL (rules)
│   ├── llm_ask.py         optional local-LLM translator (Ollama)
│   └── rl_optimizer.py    Q-learning optimizer
├── snail_cli/
│   └── main.py            interactive REPL
├── tests/                 pytest suite (ask, cache, engine integration)
├── benchmarks/
│   └── bench.py           engine vs cache benchmark
├── docs/
│   └── ARCHITECTURE.md
└── README.md
```

## SQL Support

```sql
CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)
INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30), (2, 'Bob', 20)
SELECT * FROM users
SELECT name FROM users WHERE age > 25
SELECT * FROM users WHERE name = 'Alice' AND age = 30
SELECT * FROM users ORDER BY age DESC LIMIT 5
UPDATE users SET age = 31 WHERE id = 1
DELETE FROM users WHERE id = 2
SELECT users.name, orders.amount FROM users INNER JOIN orders ON users.id = orders.user_id
SELECT users.name, orders.amount FROM users LEFT JOIN orders ON users.id = orders.user_id
SELECT COUNT(*), AVG(age), MAX(age) FROM users WHERE age > 18
SELECT region, SUM(amount) FROM sales GROUP BY region HAVING COUNT(*) > 1 ORDER BY SUM(amount) DESC LIMIT 3
```

Aggregates: `COUNT(*)`, `COUNT(col)`, `SUM`, `AVG`, `MIN`, `MAX`. NULLs are ignored except by `COUNT(*)`; an aggregate over no rows is `NULL` (`COUNT` is `0`).

```sql
CREATE INDEX idx_users_age ON users (age)
SELECT * FROM users WHERE age >= 30     -- uses the index
```

An index on an INTEGER or TEXT column speeds up `=`, `<`, `<=`, `>`, `>=` in `WHERE` for `SELECT`, `UPDATE` and `DELETE`. Results are identical with or without it.

```sql
BEGIN
INSERT INTO accounts (id, balance) VALUES (1, 100)
UPDATE accounts SET balance = 50 WHERE id = 1
ROLLBACK        -- or COMMIT
```

Transactions buffer writes until `COMMIT`, which applies them in one atomic step. Every single write statement is atomic as well, so a failing multi-row `INSERT` leaves nothing behind.

```sql
EXPLAIN SELECT * FROM users WHERE age >= 30
-- INDEX SCAN users using idx_users_age (age >= 30)
-- FILTER age >= 30
SHOW TABLES
DESCRIBE users
```

`EXPLAIN` works on `SELECT`, `UPDATE` and `DELETE` and never runs the statement.

With `GROUP BY`, every selected column must be a grouping column or an aggregate. `HAVING` filters groups, and `ORDER BY` can sort by an aggregate.

## Natural Language ASK

```
ASK show me all users
ASK find users older than 25
ASK get top 3 users ordered by age desc
ASK delete user with id 2
ASK how many users are older than 25
ASK what is the average age of users
ASK how many users per age
```

Tables are discovered automatically at startup. Use `\tables` to register extra ones.

### Optional: local LLM fallback

If the rules cannot translate a question, ASK can hand it to a local model through [Ollama](https://ollama.com). Nothing leaves your machine.

```bash
ollama pull llama3.2
python snail_cli/main.py --llm
```

```
snaildb> ASK who is the oldest person
  Generated SQL: SELECT name FROM users ORDER BY age DESC LIMIT 1  (local LLM)
```

Only a single `SELECT` is ever accepted from the model, and the engine checks it with `EXPLAIN` before running it. Configure with `SNAILDB_LLM_MODEL` (default `llama3.2`) and `SNAILDB_LLM_HOST` (default `http://localhost:11434`).

## Semantic Cache

The cache embeds each SELECT query as a vector. When a new query arrives it checks cosine similarity against all cached embeddings. Queries above a 0.92 similarity threshold return instantly without hitting the Rust engine. Writes automatically invalidate the cache.

Embeddings alone are unsafe for SQL: `WHERE id = 1` and `WHERE id = 2` look almost identical to a language model but return different rows. So a semantic hit must also pass a guard (same literals, same remaining tokens, same SELECT list). Repeats of the exact same query, ignoring case, whitespace and `;`, are matched without any embedding.

## RL Optimizer

A Q-learning agent observes every query execution and learns which strategy works best for each query type. It distinguishes between SELECT with WHERE, SELECT with JOIN, SELECT with ORDER BY, and write operations. Rewards are based on execution time and cache hits. The Q-table persists to disk so learning carries over between sessions. Use `\rl` to see what the agent has learned.

## Running SnailDB

Install dependencies:

```bash
pip install -r snail_cli/requirements.txt
```

Build the Rust extension:

```bash
cd snail_core
source .venv/Scripts/activate   # Windows (Git Bash). On Linux/macOS: source .venv/bin/activate
maturin develop
```

Run the REPL:

```bash
cd ..
python snail_cli/main.py
```

## REPL Commands

| Command | Description |
|---|---|
| Any SQL | Execute SQL directly |
| `ASK question` | Natural language query |
| `\tables name1 name2` | Register tables for ASK |
| `\cache` | Semantic cache statistics |
| `\rl` | RL optimizer statistics |
| `\quit` | Exit |

## Tests

```bash
# Rust: storage, catalog, lexer, parser, executor
cd snail_core
cargo test

# Python: ASK translator, semantic cache, end-to-end through PyO3
cd ..
maturin develop -m snail_core/Cargo.toml   # build the extension into your active venv
python -m pytest tests
```

The Python tests use a fake embedder, so they do not need the sentence-transformers model. CI runs formatting, clippy, `cargo test` and `pytest` on every push.

## Benchmarks

```bash
python benchmarks/bench.py                  # real embedding model
python benchmarks/bench.py --fake-embedder  # no model download
```

Reports mean, median and p95 latency of a full-scan query on the engine versus a semantic cache hit. Numbers depend on your machine, so run it yourself.

## Stack

| Component | Technology |
|---|---|
| Core engine | Rust |
| Storage | redb 2 |
| Python bridge | PyO3 0.23 |
| Build tool | maturin |
| Embeddings | sentence-transformers |
| Runtime | Python 3.13 |

## Version History

See [CHANGELOG.md](CHANGELOG.md).

| Version | Features |
|---|---|
| v0.1.0 | Storage, catalog, SQL parser, CREATE INSERT SELECT WHERE, semantic cache, REPL |
| v0.2.0 | UPDATE, DELETE, ORDER BY, LIMIT |
| v0.3.0 | INNER JOIN and LEFT JOIN |
| v0.4.0 | Natural language ASK command |
| v0.5.0 | Reinforcement learning query optimizer |
| v0.6.0 | CI, license, contributing guide, architecture docs |
| v0.7.0 | Python tests, cache correctness fix, benchmarks |
| v0.8.0 | Aggregates: COUNT, SUM, AVG, MIN, MAX |
| v0.9.0 | GROUP BY, HAVING, ORDER BY aggregate |
| v0.10.0 | CREATE INDEX and index-aware lookups |
| v0.11.0 | Transactions: BEGIN, COMMIT, ROLLBACK; atomic statements |
| v0.12.0 | EXPLAIN, SHOW TABLES, DESCRIBE, optional local-LLM ASK |

## Roadmap

- v1.0.0 Polish and release

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT, see [LICENSE](LICENSE).
