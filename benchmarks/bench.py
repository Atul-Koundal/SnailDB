"""Micro-benchmark: full scan vs. index lookup, and raw engine query vs. semantic cache hit.

Usage:
    python benchmarks/bench.py                 # real embedding model
    python benchmarks/bench.py --fake-embedder # no model download, measures cache overhead only
    python benchmarks/bench.py --rows 20000 --repeat 50
"""
import argparse
import os
import statistics
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import numpy as np  # noqa: E402

import snail_core  # noqa: E402
from snail_ai.semantic_cache import SemanticCache  # noqa: E402


def timed_ms(fn, repeat):
    samples = []
    for _ in range(repeat):
        t0 = time.perf_counter()
        fn()
        samples.append((time.perf_counter() - t0) * 1000)
    return samples


def summarize(label, samples):
    ordered = sorted(samples)
    p95 = ordered[max(0, int(len(ordered) * 0.95) - 1)]
    print(f"  {label:<28} mean {statistics.mean(samples):9.3f} ms   "
          f"median {statistics.median(samples):9.3f} ms   p95 {p95:9.3f} ms")
    return statistics.mean(samples)


def fake_embedder(text):
    # Deterministic, cheap stand-in so the benchmark runs without a model.
    rng = np.random.default_rng(abs(hash(text)) % (2**32))
    return rng.standard_normal(384)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--rows", type=int, default=5000)
    parser.add_argument("--repeat", type=int, default=30)
    parser.add_argument("--fake-embedder", action="store_true")
    args = parser.parse_args()

    with tempfile.TemporaryDirectory() as tmp:
        db = snail_core.SnailDB(os.path.join(tmp, "bench.redb"))
        db.execute("CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)")
        batch = 500
        for start in range(0, args.rows, batch):
            values = ", ".join(
                f"({i}, 'user{i}', {20 + i % 50})"
                for i in range(start, min(start + batch, args.rows))
            )
            db.execute(f"INSERT INTO users (id, name, age) VALUES {values}")

        # Point lookup: full scan vs. index
        lookup = f"SELECT * FROM users WHERE id = {args.rows // 2}"
        scan = summarize("point lookup, full scan", timed_ms(lambda: db.execute(lookup), args.repeat))
        db.execute("CREATE INDEX idx_users_id ON users (id)")
        idx = summarize("point lookup, indexed", timed_ms(lambda: db.execute(lookup), args.repeat))
        print(f"\n  index speedup: {scan / idx:,.1f}x\n")

        query = "SELECT * FROM users WHERE age > 60"
        print(f"\nSnailDB benchmark: {args.rows} rows, {args.repeat} repetitions\n")

        engine = summarize("engine (full scan + WHERE)", timed_ms(lambda: db.execute(query), args.repeat))

        embedder = fake_embedder if args.fake_embedder else None
        cache = SemanticCache(embedder=embedder)
        cache.execute(db, query)  # warm the cache (miss)
        hit = summarize("semantic cache hit", timed_ms(lambda: cache.execute(db, query), args.repeat))

        print(f"\n  speedup on a hit: {engine / hit:,.1f}x")
        print(f"  {cache.stats()}\n")


if __name__ == "__main__":
    main()
