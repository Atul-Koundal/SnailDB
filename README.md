# SnailDB

An AI native SQL database built from scratch in Rust with a Python AI layer. No cloud dependencies, everything runs locally.

## What is SnailDB

SnailDB is a from scratch SQL database with an AI layer on top. The Rust core handles all storage, parsing, and execution. The Python layer adds semantic caching, natural language queries, and a reinforcement learning optimizer. The name comes from the philosophy of building slow and steady, understanding every piece before moving to the next.

## Architecture

Three layers that only talk to the layer directly below them.

**snail_core** is the Rust engine. It handles persistent storage via redb, maintains table schemas in a catalog, parses SQL through a hand written lexer and recursive descent parser, and executes queries against real data on disk.

**snail_ai** is the Python AI layer. It has three components. The semantic cache embeds query text using all-MiniLM-L6-v2 and uses cosine similarity to return cached results for semantically similar queries. The ASK translator converts natural language questions into SQL using rule based pattern matching. The RL optimizer is a Q-learning agent that learns the best execution strategy for different query types over time and persists its knowledge to disk.

**snail_cli** is the Python REPL. It connects all three layers and gives you an interactive prompt to run SQL or ask questions in plain English.

## Project Structure
SnailDB/

├── snail_core/

│   ├── src/

│   │   ├── storage/       redb key value wrapper

│   │   ├── catalog/       table schema persistence

│   │   ├── sql/           lexer, parser, AST, tokens

│   │   ├── executor/      query execution engine

│   │   └── lib.rs         PyO3 bridge

│   └── Cargo.toml

├── snail_ai/

│   ├── semantic_cache.py  sentence transformer cache

│   ├── ask.py             natural language to SQL

│   └── rl_optimizer.py    Q-learning optimizer

├── snail_cli/

│   └── main.py            interactive REPL

└── README.md

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
```

## Natural Language ASK
ASK show me all users

ASK find users older than 25

ASK get top 3 users ordered by age desc

ASK delete user with id 2

Use `\tables` to register existing tables before using ASK.

## Semantic Cache

The cache embeds each SELECT query as a vector. When a new query arrives it checks cosine similarity against all cached embeddings. Queries above a 0.92 similarity threshold return instantly without hitting the Rust engine. Writes automatically invalidate the cache.

## RL Optimizer

A Q-learning agent observes every query execution and learns which strategy works best for each query type. It distinguishes between SELECT with WHERE, SELECT with JOIN, SELECT with ORDER BY, and write operations. Rewards are based on execution time and cache hits. The Q-table persists to disk so learning carries over between sessions. Use `\rl` to see what the agent has learned.

## Running SnailDB

Install dependencies:

```bash
pip install sentence-transformers maturin
```

Build the Rust extension:

```bash
cd snail_core
source .venv/Scripts/activate
maturin develop
```

Run the REPL:

```bash
cd ..
python snail_cli/main.py
```

## REPL Commands

| Command | Description |
|---------|-------------|
| Any SQL | Execute SQL directly |
| ASK question | Natural language query |
| \tables name1 name2 | Register tables for ASK |
| \cache | Semantic cache statistics |
| \rl | RL optimizer statistics |
| \quit | Exit |

## Test Suite

```bash
cd snail_core
cargo test
```

53 tests across storage, catalog, lexer, parser, and executor. All passing.

## Stack

| Component | Technology |
|-----------|------------|
| Core engine | Rust 1.96 |
| Storage | redb 2 |
| Python bridge | PyO3 0.23 |
| Build tool | maturin |
| Embeddings | sentence-transformers |
| Runtime | Python 3.13 |

## Version History

| Version | Features |
|---------|----------|
| v0.1.0 | Storage, catalog, SQL parser, CREATE INSERT SELECT WHERE, semantic cache, REPL |
| v0.2.0 | UPDATE, DELETE, ORDER BY, LIMIT |
| v0.3.0 | INNER JOIN and LEFT JOIN |
| v0.4.0 | Natural language ASK command |
| v0.5.0 | Reinforcement learning query optimizer |