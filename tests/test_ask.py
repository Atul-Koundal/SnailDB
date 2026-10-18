import pytest

from snail_ai.ask import AskTranslator

TABLES = ["users", "orders"]


@pytest.fixture
def ask():
    return AskTranslator()


@pytest.mark.parametrize(
    "question, expected",
    [
        ("show me all users", "SELECT * FROM users"),
        ("find users older than 25", "SELECT * FROM users WHERE age > 25"),
        ("find users younger than 30", "SELECT * FROM users WHERE age < 30"),
        ("list users named alice", "SELECT * FROM users WHERE name = 'alice'"),
        ("get users with id 7", "SELECT * FROM users WHERE id = 7"),
        ("list orders where amount > 100", "SELECT * FROM orders WHERE amount > 100"),
        (
            "get top 3 users ordered by age desc",
            "SELECT * FROM users ORDER BY age DESC LIMIT 3",
        ),
        ("delete user with id 2", "DELETE FROM users WHERE id = 2"),
        ("update users set age to 40 where id = 1", "UPDATE users SET age = 40 WHERE id = 1"),
    ],
)
def test_rule_based_translation(ask, question, expected):
    assert ask.translate(question, TABLES) == expected


def test_unknown_table_returns_none(ask):
    assert ask.translate("show me all widgets", TABLES) is None


def test_no_tables_registered_returns_none(ask):
    assert ask.translate("show me all users", []) is None


def test_translation_is_recorded_in_history(ask):
    ask.translate("show me all users", TABLES)
    assert ask.history[-1]["sql"] == "SELECT * FROM users"
