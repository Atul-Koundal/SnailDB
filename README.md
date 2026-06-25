SnailDB

An AI native SQL database built from scratch. Rust core engine with a Python AI layer connected via PyO3. No cloud dependencies, everything runs locally.


What is SnailDB

SnailDB is a from scratch implementation of a SQL database with an AI powered semantic caching layer. The name comes from the philosophy of building slow and steady, understanding every piece rather than reaching for existing solutions.


Architecture

The project is split into three layers that only talk to the layer directly below them.

snail_core is the Rust engine. It handles all storage via redb, maintains table schemas in a catalog, parses SQL through a hand written lexer and recursive descent parser, and executes queries against real persistent storage.

snail_ai is the Python AI layer. It embeds query text using the all-MiniLM-L6-v2 sentence transformer model and uses cosine similarity to detect semantically similar queries. Cache hits return instantly without touching the Rust engine. Writes invalidate the cache automatically.

snail_cli is the Python REPL. It connects the AI cache and the Rust engine together and gives you an interactive prompt to run SQL.


Project Structure

SnailDB
    snail_core
        src
            storage       redb key value wrapper with prefix scan and delete
            catalog       table schema persistence as JSON in redb
            sql           hand written lexer, parser, and AST
            executor      CREATE INSERT SELECT WHERE evaluation
            lib.rs        PyO3 module exposing SnailDB to Python
        Cargo.toml
    snail_ai
        semantic_cache.py     sentence transformer cache with cosine similarity
    snail_cli
        main.py               interactive REPL with timing and cache stats
    README.md


SQL Support

CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)
INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)
SELECT * FROM users
SELECT name FROM users WHERE age > 25
SELECT * FROM users WHERE name = 'Alice' AND age = 30
SELECT * FROM users WHERE age = 20 OR age = 30


Semantic Cache

The cache embeds each SELECT query as a vector using a local transformer model. When a new query arrives it is compared against all cached embeddings using cosine similarity. Queries with a similarity score above 0.92 return the cached result without hitting the Rust engine. This means queries worded differently but asking for the same data can still hit the cache.

First query     executed in 10.9ms
Second query    cache hit in 9.1ms


Running SnailDB

Install dependencies

pip install sentence-transformers maturin

Build the Rust extension

cd snail_core
maturin develop

Run the REPL

cd ..
python snail_cli/main.py

Commands inside the REPL

Type any SQL statement and press enter
Type backslash cache to see hit and miss statistics
Type quit or backslash quit to exit


Test Suite

cd snail_core
cargo test

30 tests across storage, catalog, lexer, parser, and executor. All passing.


Stack

Rust 1.96
Python 3.13
redb 2 for persistent embedded storage
PyO3 0.23 for the Rust Python bridge
sentence-transformers for semantic embeddings
maturin for building the PyO3 extension


Roadmap

UPDATE and DELETE statements
ORDER BY and LIMIT
JOIN support
Natural language ASK command
Web dashboard