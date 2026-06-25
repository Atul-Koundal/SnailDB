import time
from typing import Optional
from sentence_transformers import SentenceTransformer
import numpy as np

MODEL_NAME = "all-MiniLM-L6-v2"
SIMILARITY_THRESHOLD = 0.92


class SemanticCache:
    """
    Caches SQL query results using sentence-transformer embeddings.
    On a SELECT query:
      - Embed the query string
      - Check cosine similarity against all cached query embeddings
      - If similarity > threshold: return cached result (cache hit)
      - Else: execute via engine, store embedding + result (cache miss)
    Writes (INSERT/CREATE) bypass the cache entirely.
    """

    def __init__(self):
        print("  Loading embedding model... ", end="", flush=True)
        self.model = SentenceTransformer(MODEL_NAME)
        print("ready.")
        # Each entry: {"sql": str, "embedding": np.ndarray, "result": dict}
        self.cache: list[dict] = []
        self.hits = 0
        self.misses = 0

    def _embed(self, text: str) -> np.ndarray:
        return self.model.encode(text, convert_to_numpy=True)

    def _cosine_similarity(self, a: np.ndarray, b: np.ndarray) -> float:
        denom = np.linalg.norm(a) * np.linalg.norm(b)
        if denom == 0:
            return 0.0
        return float(np.dot(a, b) / denom)

    def _find_cached(self, embedding: np.ndarray) -> Optional[dict]:
        """Return cached result if a similar query exists, else None."""
        best_score = 0.0
        best_result = None
        for entry in self.cache:
            score = self._cosine_similarity(embedding, entry["embedding"])
            if score > best_score:
                best_score = score
                best_result = entry["result"]
        if best_score >= SIMILARITY_THRESHOLD:
            return best_result
        return None

    def _is_write(self, sql: str) -> bool:
        """Returns True for statements that modify data."""
        first_word = sql.strip().split()[0].upper()
        return first_word in ("INSERT", "CREATE", "DROP", "UPDATE", "DELETE")

    def execute(self, db, sql: str) -> tuple[dict, bool]:
        """
        Execute sql via db, using cache for SELECT queries.
        Returns (result_dict, was_cache_hit).
        db must have an .execute(sql) method (the PyO3 SnailDB object).
        """
        # Writes always bypass cache and invalidate it
        if self._is_write(sql):
            result = db.execute(sql)
            # Invalidate entire cache on writes so stale results
            # aren't returned after data changes
            self.cache.clear()
            return result, False

        # For SELECT queries, check cache first
        embedding = self._embed(sql)
        cached = self._find_cached(embedding)

        if cached is not None:
            self.hits += 1
            return cached, True

        # Cache miss — execute and store
        result = db.execute(sql)
        self.cache.append({
            "sql": sql,
            "embedding": embedding,
            "result": result,
        })
        self.misses += 1
        return result, False

    def stats(self) -> str:
        total = self.hits + self.misses
        if total == 0:
            return "Cache: no queries yet."
        rate = (self.hits / total) * 100
        return (f"Cache: {self.hits} hits / {self.misses} misses "
                f"({rate:.0f}% hit rate, {len(self.cache)} entries)")