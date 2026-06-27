import re
from typing import Optional


class AskTranslator:
    """
    Translates natural language questions into SQL.
    Two-stage approach:
      1. Rule-based regex patterns for common queries
      2. Semantic similarity fallback using query history
    """

    def __init__(self):
        # History of (natural_language, sql) pairs for similarity fallback
        self.history: list[dict] = []

    def translate(self, question: str, known_tables: list[str]) -> Optional[str]:
        """
        Translate a natural language question to SQL.
        Returns the SQL string or None if translation failed.
        """
        q = question.strip().lower()

        # Try rule-based first
        sql = self._rule_based(q, known_tables)
        if sql:
            self.history.append({"question": question, "sql": sql})
            return sql

        # Fallback: find most similar past question
        sql = self._similarity_fallback(q)
        if sql:
            return sql

        return None

    def _find_table(self, q: str, known_tables: list[str]) -> Optional[str]:
        """Find the first known table name mentioned in the question."""
        for table in known_tables:
            if table.lower() in q:
                return table
        return None

    def _extract_condition(self, q: str) -> Optional[str]:
        """
        Try to extract a WHERE condition from natural language.
        Handles patterns like:
          "where age > 25"
          "older than 25"
          "named Alice"
          "with id 1"
          "where id = 1"
        """
        # Direct where clause
        where_match = re.search(
            r'where\s+(\w+)\s*(=|>|<|>=|<=|!=|is)\s*[\'"]?(\w+)[\'"]?', q
        )
        if where_match:
            col, op, val = where_match.groups()
            op = "=" if op == "is" else op
            if val.lstrip("-").isdigit():
                return f"{col} {op} {val}"
            return f"{col} {op} '{val}'"

        # "older than N" / "younger than N"
        age_match = re.search(r'older than (\d+)', q)
        if age_match:
            return f"age > {age_match.group(1)}"

        age_match = re.search(r'younger than (\d+)', q)
        if age_match:
            return f"age < {age_match.group(1)}"

        # "named X" / "with name X"
        name_match = re.search(r'(?:named|with name)\s+[\'"]?(\w+)[\'"]?', q)
        if name_match:
            return f"name = '{name_match.group(1)}'"

        # "with id N" / "id = N" / "id is N"
        id_match = re.search(r'(?:with\s+)?id\s*(?:=|is)?\s*(\d+)', q)
        if id_match:
            return f"id = {id_match.group(1)}"

        # "amount greater than N" / "amount less than N"
        col_compare = re.search(
            r'(\w+)\s+(?:greater|more) than\s+(\d+)', q
        )
        if col_compare:
            return f"{col_compare.group(1)} > {col_compare.group(2)}"

        col_compare = re.search(
            r'(\w+)\s+(?:less|fewer) than\s+(\d+)', q
        )
        if col_compare:
            return f"{col_compare.group(1)} < {col_compare.group(2)}"

        return None

    def _extract_limit(self, q: str) -> Optional[int]:
        """Extract LIMIT from patterns like 'top 5', 'first 3', 'limit 10'."""
        limit_match = re.search(r'(?:top|first|limit)\s+(\d+)', q)
        if limit_match:
            return int(limit_match.group(1))
        return None

    def _extract_order(self, q: str) -> Optional[tuple[str, str]]:
        """Extract ORDER BY from patterns like 'order by age desc', 'sorted by name'."""
        order_match = re.search(
            r'(?:order(?:ed)? by|sort(?:ed)? by)\s+(\w+)(?:\s+(asc|desc))?', q
        )
        if order_match:
            col = order_match.group(1)
            direction = (order_match.group(2) or "asc").upper()
            return col, direction
        return None

    def _rule_based(self, q: str, known_tables: list[str]) -> Optional[str]:
        """Apply rule-based translation patterns."""
        table = self._find_table(q, known_tables)

        # ── SELECT patterns ───────────────────────────────────────────
        select_triggers = [
            "show", "get", "find", "list", "fetch",
            "select", "give me", "display", "what are",
            "all", "which"
        ]
        is_select = any(t in q for t in select_triggers)

        # ── DELETE patterns ───────────────────────────────────────────
        delete_triggers = ["delete", "remove"]
        is_delete = any(t in q for t in delete_triggers)

        # ── UPDATE patterns ───────────────────────────────────────────
        update_triggers = ["update", "change", "set", "modify"]
        is_update = any(t in q for t in update_triggers)

        if not table:
            return None

        # DELETE
        if is_delete:
            condition = self._extract_condition(q)
            if condition:
                return f"DELETE FROM {table} WHERE {condition}"
            return f"DELETE FROM {table}"

        # UPDATE — limited rule support
        if is_update:
            set_match = re.search(
                r'set\s+(\w+)\s*(?:to|=)\s*[\'"]?(\w+)[\'"]?', q
            )
            condition = self._extract_condition(q)
            if set_match:
                col, val = set_match.groups()
                val_str = val if val.lstrip("-").isdigit() else f"'{val}'"
                sql = f"UPDATE {table} SET {col} = {val_str}"
                if condition:
                    sql += f" WHERE {condition}"
                return sql

        # SELECT (default)
        if is_select or (not is_delete and not is_update):
            condition = self._extract_condition(q)
            order = self._extract_order(q)
            limit = self._extract_limit(q)

            sql = f"SELECT * FROM {table}"
            if condition:
                sql += f" WHERE {condition}"
            if order:
                sql += f" ORDER BY {order[0]} {order[1]}"
            if limit:
                sql += f" LIMIT {limit}"
            return sql

        return None

    def _similarity_fallback(self, q: str) -> Optional[str]:
        """
        Find the most similar past question using word overlap
        and return its SQL. Simple bag-of-words similarity.
        No ML needed — the semantic cache handles the heavy lifting.
        """
        if not self.history:
            return None

        q_words = set(q.split())
        best_score = 0.0
        best_sql = None

        for entry in self.history:
            past_words = set(entry["question"].lower().split())
            intersection = q_words & past_words
            union = q_words | past_words
            score = len(intersection) / len(union) if union else 0.0
            if score > best_score:
                best_score = score
                best_sql = entry["sql"]

        # Only use fallback if similarity is reasonable
        if best_score > 0.4:
            return best_sql

        return None