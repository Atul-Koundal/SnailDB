"""Optional natural-language -> SQL translation with a local LLM served by Ollama.

Everything stays on your machine: requests go to http://localhost:11434 by default.
No extra Python packages are needed (plain `urllib`).

Safety: only a single SELECT statement is accepted. Anything else the model returns
(writes, multiple statements, prose) is discarded, so the LLM can read data but never
change it.
"""
import json
import os
import re
import urllib.error
import urllib.request
from typing import Callable, Optional

DEFAULT_HOST = "http://localhost:11434"
DEFAULT_MODEL = "llama3.2"

PROMPT = """You translate questions into SQL for SnailDB, a small SQL database.

Supported SQL: SELECT with WHERE (=, !=, <, <=, >, >=, AND, OR), INNER JOIN / LEFT JOIN
(ON a.x = b.y), GROUP BY, HAVING, ORDER BY, LIMIT, and the aggregates COUNT, SUM, AVG,
MIN, MAX. Column types are INTEGER and TEXT. Text literals use single quotes.
There are no subqueries, no LIKE, no DISTINCT and no aliases.

Tables:
{schema}

Reply with exactly one SELECT statement and nothing else. No explanation, no markdown.

Question: {question}
SQL:"""

_FENCE_RE = re.compile(r"```(?:sql)?\s*(.*?)```", re.DOTALL | re.IGNORECASE)


def clean_sql(text: str) -> Optional[str]:
    """Extract one safe SELECT statement from raw model output, or return None."""
    if not text:
        return None
    fenced = _FENCE_RE.search(text)
    sql = (fenced.group(1) if fenced else text).strip()
    sql = " ".join(sql.split())            # collapse whitespace / newlines
    sql = sql.rstrip(";").strip()
    if not sql.upper().startswith("SELECT"):
        return None
    if ";" in sql:                          # more than one statement
        return None
    return sql


def format_schema(schemas: dict) -> str:
    """{'users': [('id', 'INTEGER'), ...]} -> 'users(id INTEGER, ...)' lines."""
    lines = []
    for table, columns in schemas.items():
        cols = ", ".join(f"{name} {ctype}" for name, ctype in columns)
        lines.append(f"- {table}({cols})")
    return "\n".join(lines) if lines else "(no tables)"


class LLMTranslator:
    def __init__(
        self,
        model: Optional[str] = None,
        host: Optional[str] = None,
        timeout: float = 60.0,
        opener: Optional[Callable] = None,
    ):
        self.model = model or os.environ.get("SNAILDB_LLM_MODEL", DEFAULT_MODEL)
        self.host = (host or os.environ.get("SNAILDB_LLM_HOST", DEFAULT_HOST)).rstrip("/")
        self.timeout = timeout
        self._open = opener or urllib.request.urlopen

    def is_available(self) -> bool:
        """True if the Ollama server answers."""
        try:
            with self._open(f"{self.host}/api/tags", timeout=3) as resp:
                return getattr(resp, "status", 200) == 200
        except (urllib.error.URLError, OSError, ValueError):
            return False

    def build_prompt(self, question: str, schemas: dict) -> str:
        return PROMPT.format(schema=format_schema(schemas), question=question.strip())

    def translate(self, question: str, schemas: Optional[dict] = None) -> Optional[str]:
        """Return a SELECT statement for `question`, or None if the model is unreachable
        or its answer is not a single SELECT."""
        payload = json.dumps(
            {
                "model": self.model,
                "prompt": self.build_prompt(question, schemas or {}),
                "stream": False,
                "options": {"temperature": 0},
            }
        ).encode()
        request = urllib.request.Request(
            f"{self.host}/api/generate",
            data=payload,
            headers={"Content-Type": "application/json"},
        )
        try:
            with self._open(request, timeout=self.timeout) as resp:
                body = json.loads(resp.read().decode())
        except (urllib.error.URLError, OSError, ValueError):
            return None
        return clean_sql(body.get("response", ""))
