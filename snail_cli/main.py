import sys
import os
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import snail_core
from snail_ai.semantic_cache import SemanticCache
from snail_ai.ask import AskTranslator
from snail_ai.llm_ask import LLMTranslator

DB_PATH = "snaildb.redb"

BANNER = r"""
 _____             _ _  ____  ____
/ ____|           (_) ||  _ \|  _ \
| (___  _ __   __ _ _| || | | | |_) |
 \___ \| '_ \ / _` | | || | | |  _ 
 ____) | | | | (_| | | || |_| | |_) |
|_____/|_| |_|\__,_|_|_||____/|____/

SnailDB v1.0.0 — slow and steady, smart and ready
Type SQL to query. Type ASK <question> for natural language.
Tables are discovered automatically; \tables <name> registers extra ones.
Start with --llm to let ASK fall back to a local LLM (Ollama).
Type \help for commands. Type \quit to exit.
"""


def print_result(result: dict):
    if result["message"]:
        print(f"  {result['message']}")
        return

    columns = result["columns"]
    rows    = result["rows"]

    if not columns:
        print("  (no results)")
        return

    widths = [len(c) for c in columns]
    for row in rows:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(str(cell)) if cell is not None else 4)

    header    = " | ".join(c.ljust(widths[i]) for i, c in enumerate(columns))
    separator = "-+-".join("-" * w for w in widths)
    print(f"  {header}")
    print(f"  {separator}")

    for row in rows:
        cells = [
            (str(cell) if cell is not None else "NULL").ljust(widths[i])
            for i, cell in enumerate(row)
        ]
        print(f"  {' | '.join(cells)}")

    count = len(rows)
    print(f"\n  {count} row{'s' if count != 1 else ''} returned.")


HELP = """
  SQL
    CREATE TABLE t (id INTEGER, name TEXT)      CREATE INDEX idx ON t (col)
    INSERT INTO t (id, name) VALUES (1, 'a')    UPDATE t SET col = v WHERE ...
    SELECT ... [JOIN] [WHERE] [GROUP BY] [HAVING] [ORDER BY] [LIMIT]
    DELETE FROM t WHERE ...
    BEGIN / COMMIT / ROLLBACK                   EXPLAIN <select|update|delete>
    SHOW TABLES                                 DESCRIBE t

  Natural language
    ASK how many users per age                  (add --llm for a local-LLM fallback)

  REPL
    \\help      this text
    \\tables    list known tables; \\tables name registers extra ones
    \\cache     semantic cache statistics
    \\quit      exit
"""


def discover_tables(db) -> list[str]:
    """Names of all tables already in the database."""
    return [row[0] for row in db.execute("SHOW TABLES")["rows"]]


def load_schemas(db, tables: list[str]) -> dict:
    """{table: [(column, type), ...]}, used to give the LLM context."""
    schemas = {}
    for table in tables:
        try:
            rows = db.execute(f"DESCRIBE {table}")["rows"]
        except RuntimeError:
            continue
        schemas[table] = [(r[0], r[1]) for r in rows]
    return schemas


def main():
    print(BANNER)

    try:
        db = snail_core.SnailDB(DB_PATH)
    except RuntimeError as e:
        print(f"Failed to open database: {e}")
        sys.exit(1)

    cache = SemanticCache()

    llm = None
    if "--llm" in sys.argv or os.environ.get("SNAILDB_LLM") == "1":
        llm = LLMTranslator()
        if llm.is_available():
            print(f"  Local LLM ready ({llm.model}). ASK uses it when the rules do not match.")
        else:
            print("  Local LLM not reachable (is Ollama running?). Using rule-based ASK only.")
            llm = None

    ask = AskTranslator(llm=llm)
    known_tables: list[str] = discover_tables(db)
    if known_tables:
        print(f"  Found tables: {known_tables}")
    print()

    while True:
        try:
            prompt = "snaildb*> " if cache.in_transaction else "snaildb> "
            user_input = input(prompt).strip()
        except (EOFError, KeyboardInterrupt):
            print("\nBye!")
            break

        if not user_input:
            continue

        if user_input.lower() in ("\\quit", "\\q", "exit", "quit"):
            print("Bye!")
            break

        if user_input.lower() in ("\\help", "\\h", "help"):
            print(HELP)
            continue

        if user_input.lower() == "\\cache":
            print(f"  {cache.stats()}")
            print()
            continue

        # ── \tables command — register existing tables for ASK ────────
        if user_input.lower().startswith("\\tables"):
            parts = user_input.split()[1:]
            if parts:
                for p in parts:
                    if p not in known_tables:
                        known_tables.append(p)
                print(f"  Known tables: {known_tables}")
            else:
                print(f"  Known tables: {known_tables if known_tables else '(none)'}")
            print()
            continue

        # ── ASK command ───────────────────────────────────────────────
        if user_input.upper().startswith("ASK "):
            question = user_input[4:].strip()
            schemas = load_schemas(db, known_tables) if llm else None
            sql = ask.translate(question, known_tables, schemas)
            if sql and ask.last_source == "llm":
                # The model can be wrong: let the engine check the statement first.
                try:
                    db.execute("EXPLAIN " + sql)
                except RuntimeError as e:
                    print(f"  The LLM produced SQL the engine rejected ({e}): {sql}")
                    print()
                    continue
            if sql:
                source = "  (local LLM)" if ask.last_source == "llm" else ""
                print(f"  Generated SQL: {sql}{source}")
                user_input = sql
            else:
                print(f"  Could not translate: '{question}'")
                if not known_tables:
                    print(f"  No tables registered. Use: \\tables users orders ...")
                else:
                    print(f"  Known tables: {known_tables}")
                    print(f"  Try mentioning a table name in your question.")
                print()
                continue

        # ── Track CREATE TABLE so ASK knows what tables exist ─────────
        if user_input.upper().startswith("CREATE TABLE"):
            words = user_input.split()
            if len(words) >= 3:
                table_name = words[2].strip("(")
                if table_name not in known_tables:
                    known_tables.append(table_name)

        # ── Execute ───────────────────────────────────────────────────
        try:
            t0 = time.perf_counter()
            result, hit = cache.execute(db, user_input)
            elapsed = (time.perf_counter() - t0) * 1000

            print_result(result)

            if hit:
                print(f"  cache hit  ({elapsed:.3f}ms)")
            else:
                print(f"  executed   ({elapsed:.3f}ms)")

        except RuntimeError as e:
            print(f"  Error: {e}")

        print()


if __name__ == "__main__":
    main()