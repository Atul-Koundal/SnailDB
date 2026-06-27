import sys
import os
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import snail_core
from snail_ai.semantic_cache import SemanticCache
from snail_ai.ask import AskTranslator

DB_PATH = "snaildb.redb"

BANNER = r"""
 _____             _ _  ____  ____
/ ____|           (_) ||  _ \|  _ \
| (___  _ __   __ _ _| || | | | |_) |
 \___ \| '_ \ / _` | | || | | |  _ 
 ____) | | | | (_| | | || |_| | |_) |
|_____/|_| |_|\__,_|_|_||____/|____/

SnailDB v0.4.0 — slow and steady, smart and ready
Type SQL to query. Type ASK <question> for natural language.
Type \cache for cache stats. Type \quit to exit.
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


def get_known_tables(db) -> list[str]:
    """Get list of tables from the database via SHOW TABLES workaround."""
    try:
        # We use the catalog scan — try a known pattern
        # For now return empty list; user can tell ASK which table
        return []
    except Exception:
        return []


def main():
    print(BANNER)

    try:
        db = snail_core.SnailDB(DB_PATH)
    except RuntimeError as e:
        print(f"Failed to open database: {e}")
        sys.exit(1)

    cache = SemanticCache()
    ask = AskTranslator()
    known_tables: list[str] = []
    print()

    while True:
        try:
            user_input = input("snaildb> ").strip()
        except (EOFError, KeyboardInterrupt):
            print("\nBye!")
            break

        if not user_input:
            continue

        if user_input.lower() in ("\\quit", "\\q", "exit", "quit"):
            print("Bye!")
            break

        if user_input.lower() == "\\cache":
            print(f"  {cache.stats()}")
            print()
            continue

        # ── ASK command ───────────────────────────────────────────────
        if user_input.upper().startswith("ASK "):
            question = user_input[4:].strip()
            sql = ask.translate(question, known_tables)
            if sql:
                print(f"  Generated SQL: {sql}")
                user_input = sql
            else:
                print(f"  Could not translate: '{question}'")
                print(f"  Tip: mention a table name in your question.")
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