import sys
import os

# The compiled snail_core .pyd sits one level up after `maturin develop`
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import snail_core

DB_PATH = "snaildb.redb"

BANNER = """
 _____             _ _  ____  ____
/ ____|           (_) ||  _ \|  _ \\
| (___  _ __   __ _ _| || | | | |_) |
 \___ \| '_ \ / _` | | || | | |  _ 
 ____) | | | | (_| | | || |_| | |_) |
|_____/|_| |_|\__,_|_|_||____/|____/

SnailDB v0.1.0 — slow and steady, smart and ready 🐌
Type SQL to query. Type \\quit to exit.
"""

def print_result(result: dict):
    """Pretty-print a QueryResult dict."""
    if result["message"]:
        print(f"  ✓ {result['message']}")
        return

    columns = result["columns"]
    rows    = result["rows"]

    if not columns:
        print("  (no results)")
        return

    # Calculate column widths
    widths = [len(c) for c in columns]
    for row in rows:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(str(cell)) if cell is not None else 4)

    # Header
    header = " | ".join(c.ljust(widths[i]) for i, c in enumerate(columns))
    separator = "-+-".join("-" * w for w in widths)
    print(f"  {header}")
    print(f"  {separator}")

    # Rows
    for row in rows:
        cells = []
        for i, cell in enumerate(row):
            val = str(cell) if cell is not None else "NULL"
            cells.append(val.ljust(widths[i]))
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

        try:
            result = db.execute(sql)
            print_result(result)
        except RuntimeError as e:
            print(f"  ✗ Error: {e}")

        print()


if __name__ == "__main__":
    main()