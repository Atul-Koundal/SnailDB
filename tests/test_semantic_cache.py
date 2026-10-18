import numpy as np
import pytest

from snail_ai.semantic_cache import SemanticCache, normalize_sql, signature


class FakeDB:
    """Stands in for the PyO3 SnailDB object and counts engine calls."""

    def __init__(self):
        self.calls = []

    def execute(self, sql):
        self.calls.append(sql)
        return {"columns": ["c"], "rows": [[sql]], "message": None}


def constant_embedder(_text):
    # Every query embeds to the same vector, i.e. similarity is always 1.0.
    # This is the worst case for a purely embedding-based cache.
    return np.array([1.0, 0.0, 0.0])


@pytest.fixture
def db():
    return FakeDB()


@pytest.fixture
def cache():
    return SemanticCache(embedder=constant_embedder)


def test_normalize_ignores_case_whitespace_and_semicolon():
    assert normalize_sql("SELECT  *\nFROM Users ;") == normalize_sql("select * from users")


def test_normalize_keeps_string_literal_case():
    assert normalize_sql("SELECT * FROM t WHERE n = 'Bob'") != normalize_sql(
        "SELECT * FROM t WHERE n = 'bob'"
    )


def test_exact_repeat_is_a_hit(cache, db):
    _, hit1 = cache.execute(db, "SELECT * FROM users")
    _, hit2 = cache.execute(db, "select  *  from users;")
    assert (hit1, hit2) == (False, True)
    assert len(db.calls) == 1
    assert cache.exact_hits == 1


def test_different_literals_never_share_a_result(cache, db):
    """Regression: embeddings make `id = 1` and `id = 2` look identical."""
    cache.execute(db, "SELECT * FROM users WHERE id = 1")
    _, hit = cache.execute(db, "SELECT * FROM users WHERE id = 2")
    assert hit is False
    assert len(db.calls) == 2


def test_different_operator_is_a_miss(cache, db):
    cache.execute(db, "SELECT * FROM users WHERE age > 25")
    _, hit = cache.execute(db, "SELECT * FROM users WHERE age < 25")
    assert hit is False


def test_different_select_list_order_is_a_miss(cache, db):
    cache.execute(db, "SELECT id, name FROM users")
    _, hit = cache.execute(db, "SELECT name, id FROM users")
    assert hit is False


def test_semantic_hit_for_reordered_predicates(db):
    """Equivalent queries with different text can still share a result."""
    cache = SemanticCache(embedder=constant_embedder)
    cache.execute(db, "SELECT * FROM users WHERE a = 1 AND b = 1")
    _, hit = cache.execute(db, "SELECT * FROM users WHERE b = 1 AND a = 1")
    assert hit is True
    assert cache.exact_hits == 0  # it was a semantic hit, not an exact one
    assert len(db.calls) == 1


def test_low_similarity_is_a_miss(db):
    vectors = {"q1": np.array([1.0, 0.0]), "q2": np.array([0.0, 1.0])}
    cache = SemanticCache(embedder=lambda t: vectors[t])
    cache.execute(db, "q1")
    _, hit = cache.execute(db, "q2")
    assert hit is False


def test_write_clears_cache(cache, db):
    cache.execute(db, "SELECT * FROM users")
    cache.execute(db, "INSERT INTO users (id) VALUES (1)")
    assert cache.cache == []
    _, hit = cache.execute(db, "SELECT * FROM users")
    assert hit is False


@pytest.mark.parametrize("sql", ["INSERT INTO t (a) VALUES (1)", "update t set a = 1", "DELETE FROM t", "CREATE TABLE t (a INTEGER)"])
def test_is_write(cache, sql):
    assert cache._is_write(sql)


def test_select_is_not_write(cache):
    assert not cache._is_write("SELECT * FROM t")


def test_empty_sql_does_not_crash(cache):
    assert cache._is_write("") is False
    assert cache._is_write("   ") is False


def test_transaction_results_are_not_cached(cache, db):
    cache.execute(db, "BEGIN")
    cache.execute(db, "SELECT * FROM users")
    assert cache.cache == []
    cache.execute(db, "ROLLBACK")
    _, hit = cache.execute(db, "SELECT * FROM users")
    assert hit is False
    assert len(cache.cache) == 1


def test_max_entries_evicts_oldest(db):
    cache = SemanticCache(embedder=constant_embedder, max_entries=2)
    for i in range(3):
        cache.execute(db, f"SELECT * FROM t{i}")
    assert len(cache.cache) == 2
    assert cache.cache[0]["sql"] == "SELECT * FROM t1"


def test_stats_string(cache, db):
    assert "no queries" in cache.stats()
    cache.execute(db, "SELECT * FROM users")
    cache.execute(db, "SELECT * FROM users")
    assert "1 hits" in cache.stats()


def test_signature_ignores_case_and_spacing():
    assert signature("SELECT * FROM users WHERE age > 25") == signature(
        "select  *  from users where age > 25;"
    )
