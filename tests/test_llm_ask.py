import io
import json
import urllib.error

import pytest

from snail_ai.ask import AskTranslator
from snail_ai.llm_ask import LLMTranslator, clean_sql, format_schema

SCHEMAS = {"users": [("id", "INTEGER"), ("name", "TEXT"), ("age", "INTEGER")]}


class FakeResponse:
    def __init__(self, payload, status=200):
        self._body = json.dumps(payload).encode()
        self.status = status

    def read(self):
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


def opener_returning(text):
    calls = []

    def opener(request, timeout=None):
        calls.append(request)
        return FakeResponse({"response": text})

    opener.calls = calls
    return opener


def failing_opener(request, timeout=None):
    raise urllib.error.URLError("connection refused")


# ── clean_sql ─────────────────────────────────────────────────────────

@pytest.mark.parametrize(
    "raw, expected",
    [
        ("SELECT * FROM users", "SELECT * FROM users"),
        ("SELECT * FROM users;", "SELECT * FROM users"),
        ("  select name\nFROM   users\n", "select name FROM users"),
        ("```sql\nSELECT name FROM users WHERE age > 30\n```", "SELECT name FROM users WHERE age > 30"),
        ("Sure! ```SELECT 1 FROM t``` hope that helps", "SELECT 1 FROM t"),
    ],
)
def test_clean_sql_accepts_a_single_select(raw, expected):
    assert clean_sql(raw) == expected


@pytest.mark.parametrize(
    "raw",
    [
        "",
        "DROP TABLE users",
        "DELETE FROM users",
        "UPDATE users SET age = 1",
        "SELECT * FROM users; DELETE FROM users",
        "Here is the query you wanted.",
    ],
)
def test_clean_sql_rejects_everything_else(raw):
    assert clean_sql(raw) is None


def test_format_schema():
    text = format_schema(SCHEMAS)
    assert "users(id INTEGER, name TEXT, age INTEGER)" in text
    assert format_schema({}) == "(no tables)"


# ── LLMTranslator ─────────────────────────────────────────────────────

def test_translate_sends_schema_and_question_to_ollama():
    opener = opener_returning("SELECT name FROM users WHERE age > 30")
    llm = LLMTranslator(model="test-model", host="http://example:1234/", opener=opener)
    sql = llm.translate("who is older than thirty?", SCHEMAS)
    assert sql == "SELECT name FROM users WHERE age > 30"

    request = opener.calls[0]
    assert request.full_url == "http://example:1234/api/generate"
    body = json.loads(request.data)
    assert body["model"] == "test-model"
    assert body["stream"] is False
    assert "users(id INTEGER" in body["prompt"]
    assert "who is older than thirty?" in body["prompt"]


def test_translate_returns_none_when_server_is_down():
    llm = LLMTranslator(opener=failing_opener)
    assert llm.translate("anything", SCHEMAS) is None
    assert llm.is_available() is False


def test_translate_discards_unsafe_model_output():
    llm = LLMTranslator(opener=opener_returning("DELETE FROM users"))
    assert llm.translate("remove everyone", SCHEMAS) is None


def test_translate_handles_malformed_json():
    class Bad:
        status = 200

        def read(self):
            return b"not json"

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

    llm = LLMTranslator(opener=lambda r, timeout=None: Bad())
    assert llm.translate("q", SCHEMAS) is None


def test_is_available_true_on_200():
    llm = LLMTranslator(opener=lambda r, timeout=None: FakeResponse({"models": []}))
    assert llm.is_available() is True


def test_model_and_host_come_from_environment(monkeypatch):
    monkeypatch.setenv("SNAILDB_LLM_MODEL", "qwen2.5")
    monkeypatch.setenv("SNAILDB_LLM_HOST", "http://box:9999")
    llm = LLMTranslator()
    assert (llm.model, llm.host) == ("qwen2.5", "http://box:9999")


# ── AskTranslator integration ─────────────────────────────────────────

def test_rules_win_over_the_llm():
    opener = opener_returning("SELECT 1 FROM users")
    ask = AskTranslator(llm=LLMTranslator(opener=opener))
    assert ask.translate("show me all users", ["users"], SCHEMAS) == "SELECT * FROM users"
    assert ask.last_source == "rules"
    assert opener.calls == []  # the LLM was never contacted


def test_llm_is_used_when_rules_do_not_match():
    llm = LLMTranslator(opener=opener_returning("SELECT name FROM users ORDER BY age DESC LIMIT 1"))
    ask = AskTranslator(llm=llm)
    sql = ask.translate("who is the oldest person", ["users"], SCHEMAS)
    assert sql == "SELECT name FROM users ORDER BY age DESC LIMIT 1"
    assert ask.last_source == "llm"
    assert ask.history[-1]["sql"] == sql


def test_without_an_llm_unmatched_questions_still_fail():
    ask = AskTranslator()
    assert ask.translate("who is the oldest person", ["users"], SCHEMAS) is None
    assert ask.last_source is None


def test_llm_outage_falls_through_to_history_or_none():
    ask = AskTranslator(llm=LLMTranslator(opener=failing_opener))
    assert ask.translate("who is the oldest person", ["users"], SCHEMAS) is None
