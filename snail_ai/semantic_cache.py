import re
from typing import Callable, Optional

import numpy as np

MODEL_NAME = "all-MiniLM-L6-v2"
SIMILARITY_THRESHOLD = 0.92

# Statements that change data (or transaction state) and therefore invalidate the cache.
WRITE_KEYWORDS = {
    "INSERT", "CREATE", "DROP", "UPDATE", "DELETE",
    "BEGIN", "COMMIT", "ROLLBACK",
}

_TOKEN_RE = re.compile(r"'[^']*'|-?\d+|<=|>=|!=|[=<>]|\w+|[^\s\w]")
_LITERAL_RE = re.compile(r"^('[^']*'|-?\d+)$")
_PUNCT = {",", "(", ")", ";", "."}


def normalize_sql(sql: str) -> str:
    """Canonical form used for exact matching: case, whitespace and `;` are ignored,
    string literals are kept verbatim."""
    parts = []
    for tok in _TOKEN_RE.findall(sql):
        if tok == ";":
            continue
        parts.append(tok if tok.startswith("'") else tok.lower())
    return " ".join(parts)


def _tokens(sql: str) -> list[str]:
    return [t for t in _TOKEN_RE.findall(sql) if t != ";"]


def signature(sql: str) -> tuple:
    """Guard used before accepting a *semantic* hit.

    Embeddings treat `WHERE id = 1` and `WHERE id = 2` as nearly identical, but they
    return different rows. Two queries may only share a cached result if they have
    the same ordered literals, the same bag of remaining tokens, and the same
    SELECT list (output column order matters).
    """
    toks = _tokens(sql)
    literals = tuple(t for t in toks if _LITERAL_RE.match(t))
    rest = tuple(sorted(t.lower() for t in toks if not _LITERAL_RE.match(t) and t not in _PUNCT))

    select_list: tuple = ()
    lowered = [t.lower() for t in toks]
    if "select" in lowered and "from" in lowered:
        start, end = lowered.index("select") + 1, lowered.index("from")
        select_list = tuple(lowered[start:end])
    return literals, rest, select_list


def _default_embedder() -> Callable[[str], np.ndarray]:
    # Imported lazily so the rest of SnailDB (and the test suite) does not need
    # sentence-transformers or a model download.
    from sentence_transformers import SentenceTransformer

    print("  Loading embedding model... ", end="", flush=True)
    model = SentenceTransformer(MODEL_NAME)
    print("ready.")
    return lambda text: model.encode(text, convert_to_numpy=True)


class SemanticCache:
    """
    Caches SQL query results.

    On a SELECT:
      1. Exact match on the normalized query (always safe)  -> hit
      2. Otherwise embed the query and compare with cached embeddings. A hit
         needs cosine similarity >= threshold AND a matching signature (see
         `signature`), so near-identical queries with different literals never
         collide.
      3. Otherwise execute on the engine and store the result.

    Writes (INSERT/UPDATE/DELETE/CREATE/DROP) and transaction control statements
    bypass the cache and clear it. While a transaction is open nothing is cached,
    because its results may be rolled back.
    """

    def __init__(
        self,
        embedder: Optional[Callable[[str], np.ndarray]] = None,
        threshold: float = SIMILARITY_THRESHOLD,
        max_entries: int = 1000,
    ):
        self._embed_fn = embedder if embedder is not None else _default_embedder()
        self.threshold = threshold
        self.max_entries = max_entries
        # Each entry: {"sql", "key", "sig", "embedding", "result"}
        self.cache: list[dict] = []
        self.hits = 0
        self.exact_hits = 0
        self.misses = 0
        self.in_transaction = False

    # ── helpers ───────────────────────────────────────────────────────
    def _embed(self, text: str) -> np.ndarray:
        return np.asarray(self._embed_fn(text), dtype=float)

    @staticmethod
    def _cosine_similarity(a: np.ndarray, b: np.ndarray) -> float:
        denom = np.linalg.norm(a) * np.linalg.norm(b)
        if denom == 0:
            return 0.0
        return float(np.dot(a, b) / denom)

    @staticmethod
    def _first_word(sql: str) -> str:
        words = sql.strip().split()
        return words[0].upper().rstrip(";") if words else ""

    def _is_write(self, sql: str) -> bool:
        """True for statements that modify data or transaction state."""
        return self._first_word(sql) in WRITE_KEYWORDS

    def _find_exact(self, key: str) -> Optional[dict]:
        for entry in self.cache:
            if entry["key"] == key:
                return entry["result"]
        return None

    def _find_semantic(self, embedding: np.ndarray, sig: tuple) -> Optional[dict]:
        best_score, best_result = 0.0, None
        for entry in self.cache:
            if entry["sig"] != sig:
                continue
            score = self._cosine_similarity(embedding, entry["embedding"])
            if score > best_score:
                best_score, best_result = score, entry["result"]
        return best_result if best_score >= self.threshold else None

    # ── public API ────────────────────────────────────────────────────
    def execute(self, db, sql: str) -> tuple[dict, bool]:
        """Execute `sql` on `db` (an object with `.execute(sql)`), using the cache for
        SELECTs. Returns (result_dict, was_cache_hit)."""
        first = self._first_word(sql)

        if first == "BEGIN":
            result = db.execute(sql)
            self.in_transaction = True
            self.cache.clear()
            return result, False

        if first in ("COMMIT", "ROLLBACK"):
            try:
                result = db.execute(sql)
            finally:
                self.in_transaction = False
                self.cache.clear()
            return result, False

        if self._is_write(sql):
            result = db.execute(sql)
            self.cache.clear()
            return result, False

        # Inside a transaction results may be rolled back, so never cache them.
        if self.in_transaction:
            return db.execute(sql), False

        key = normalize_sql(sql)
        cached = self._find_exact(key)
        if cached is not None:
            self.hits += 1
            self.exact_hits += 1
            return cached, True

        embedding = self._embed(sql)
        sig = signature(sql)
        cached = self._find_semantic(embedding, sig)
        if cached is not None:
            self.hits += 1
            return cached, True

        result = db.execute(sql)
        self.cache.append(
            {"sql": sql, "key": key, "sig": sig, "embedding": embedding, "result": result}
        )
        if len(self.cache) > self.max_entries:
            self.cache.pop(0)  # evict the oldest entry
        self.misses += 1
        return result, False

    def stats(self) -> str:
        total = self.hits + self.misses
        if total == 0:
            return "Cache: no queries yet."
        rate = (self.hits / total) * 100
        return (
            f"Cache: {self.hits} hits ({self.exact_hits} exact) / {self.misses} misses "
            f"({rate:.0f}% hit rate, {len(self.cache)} entries)"
        )
