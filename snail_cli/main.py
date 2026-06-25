import sys
import os
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import snail_core
from snail_ai.semantic_cache import SemanticCache

DB_PATH = "snaildb.redb"

BANNER = r"""
 _____             _ _  ____  ____
/ ____|           (_) ||  _ \|  _ \
| (___  _ __   __ _ _| || | | | |_) |
 \___ \| '_ \ / _` | | || | | |  _ 
 ____) | | | | (_| | | || |_| | |_) |
|_____/|_| |_|\__,_|_|_||____/|____/

SnailDB v0.1.0 — slow and steady, smart and ready 🐌
Type SQL to query. Type \quit to exit.
Type \cache to see cache stats.
"""


def print_result(result: dict):
    if result["message"]:
        print(f"  ✓ {result['message']}")
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


def main():
    print(BANNER)

    try:
        db = snail_core.SnailDB(DB_PATH)
    except RuntimeError as e:
        print(f"Failed to open database: {e}")
        sys.exit(1)

    cache = SemanticCache()
    print()

    while True:
        try:
            sql = input("snaildb> ").strip()
        except (EOFError, KeyboardInterrupt):
            print("\nBye!")
            break

        if not sql:
            continue

        if sql.lower() in ("\\quit", "\\q", "exit", "quit"):
            print("Bye!")
            break

        if sql.lower() == "\\cache":
            print(f"  {cache.stats()}")
            print()
            continue

        try:
            t0 = time.perf_counter()
            result, hit = cache.execute(db, sql)
            elapsed = (time.perf_counter() - t0) * 1000

            print_result(result)

            if hit:
                print(f"  ⚡ cache hit  ({elapsed:.3f}ms)")
            else:
                print(f"  🐌 executed   ({elapsed:.3f}ms)")

        except RuntimeError as e:
            print(f"  ✗ Error: {e}")

        print()


if __name__ == "__main__":
    main()