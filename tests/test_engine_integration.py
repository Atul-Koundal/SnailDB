"""End-to-end tests through the PyO3 bridge. Skipped if the extension is not built
(run `maturin develop` in snail_core first)."""
import pytest

snail_core = pytest.importorskip("snail_core")


@pytest.fixture
def db(tmp_path):
    return snail_core.SnailDB(str(tmp_path / "test.redb"))


def test_create_insert_select_roundtrip(db):
    db.execute("CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)")
    db.execute("INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30), (2, 'Bob', 20)")
    result = db.execute("SELECT name FROM users WHERE age > 25")
    assert result["columns"] == ["name"]
    assert result["rows"] == [["Alice"]]


def test_message_results(db):
    result = db.execute("CREATE TABLE t (id INTEGER)")
    assert "created" in result["message"]


def test_sql_errors_raise_runtime_error(db):
    with pytest.raises(RuntimeError):
        db.execute("SELECT * FROM missing_table")
    with pytest.raises(RuntimeError):
        db.execute("THIS IS NOT SQL")


def test_data_persists_across_reopen(tmp_path):
    path = str(tmp_path / "persist.redb")
    first = snail_core.SnailDB(path)
    first.execute("CREATE TABLE t (id INTEGER)")
    first.execute("INSERT INTO t (id) VALUES (42)")
    del first
    second = snail_core.SnailDB(path)
    assert second.execute("SELECT * FROM t")["rows"] == [["42"]]


def test_cache_over_real_engine(db):
    import numpy as np

    from snail_ai.semantic_cache import SemanticCache

    cache = SemanticCache(embedder=lambda _t: np.array([1.0, 0.0]))
    cache.execute(db, "CREATE TABLE t (id INTEGER)")
    cache.execute(db, "INSERT INTO t (id) VALUES (1)")
    first, hit1 = cache.execute(db, "SELECT * FROM t")
    second, hit2 = cache.execute(db, "SELECT * FROM t")
    assert (hit1, hit2) == (False, True)
    assert first == second
    cache.execute(db, "INSERT INTO t (id) VALUES (2)")
    third, hit3 = cache.execute(db, "SELECT * FROM t")
    assert hit3 is False
    assert len(third["rows"]) == 2


def test_aggregates_through_bridge(db):
    db.execute("CREATE TABLE users (id INTEGER, age INTEGER)")
    db.execute("INSERT INTO users (id, age) VALUES (1, 30), (2, 20), (3, NULL)")
    result = db.execute("SELECT COUNT(*), COUNT(age), SUM(age), AVG(age) FROM users")
    assert result["columns"] == ["COUNT(*)", "COUNT(age)", "SUM(age)", "AVG(age)"]
    assert result["rows"] == [["3", "2", "50", "25.0"]]


def test_group_by_having_through_bridge(db):
    db.execute("CREATE TABLE sales (region TEXT, amount INTEGER)")
    db.execute(
        "INSERT INTO sales (region, amount) VALUES "
        "('north', 10), ('north', 20), ('south', 5), ('east', 100)"
    )
    result = db.execute(
        "SELECT region, SUM(amount) FROM sales GROUP BY region "
        "HAVING COUNT(*) >= 1 ORDER BY SUM(amount) DESC LIMIT 2"
    )
    assert result["columns"] == ["region", "SUM(amount)"]
    assert result["rows"] == [["east", "100"], ["north", "30"]]


def test_index_through_bridge(db):
    db.execute("CREATE TABLE t (id INTEGER, name TEXT)")
    db.execute("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')")
    message = db.execute("CREATE INDEX idx_id ON t (id)")["message"]
    assert "3 row(s) indexed" in message
    db.execute("INSERT INTO t (id, name) VALUES (4, 'd')")
    assert db.execute("SELECT name FROM t WHERE id >= 3")["rows"] == [["c"], ["d"]]
    with pytest.raises(RuntimeError):
        db.execute("CREATE INDEX idx_id ON t (name)")


def test_transactions_through_bridge(db):
    db.execute("CREATE TABLE t (id INTEGER)")
    db.execute("BEGIN")
    db.execute("INSERT INTO t (id) VALUES (1), (2)")
    assert len(db.execute("SELECT * FROM t")["rows"]) == 2
    db.execute("ROLLBACK")
    assert db.execute("SELECT * FROM t")["rows"] == []

    db.execute("BEGIN")
    db.execute("INSERT INTO t (id) VALUES (3)")
    db.execute("COMMIT")
    assert db.execute("SELECT * FROM t")["rows"] == [["3"]]

    with pytest.raises(RuntimeError):
        db.execute("COMMIT")  # no active transaction


def test_cache_never_serves_rolled_back_rows(db):
    import numpy as np

    from snail_ai.semantic_cache import SemanticCache

    cache = SemanticCache(embedder=lambda _t: np.array([1.0, 0.0]))
    cache.execute(db, "CREATE TABLE t (id INTEGER)")
    cache.execute(db, "BEGIN")
    cache.execute(db, "INSERT INTO t (id) VALUES (1)")
    inside, _ = cache.execute(db, "SELECT * FROM t")
    assert len(inside["rows"]) == 1
    cache.execute(db, "ROLLBACK")
    after, hit = cache.execute(db, "SELECT * FROM t")
    assert hit is False
    assert after["rows"] == []


def test_failed_multi_row_insert_is_atomic(db):
    db.execute("CREATE TABLE t (id INTEGER)")
    with pytest.raises(RuntimeError):
        db.execute("INSERT INTO t (id) VALUES (1), ('oops')")
    assert db.execute("SELECT * FROM t")["rows"] == []
