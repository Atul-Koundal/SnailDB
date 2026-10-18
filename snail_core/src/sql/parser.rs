use super::ast::*;
use super::token::Token;
use crate::catalog::ColType;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("expected {expected}, got {got:?}")]
    Expected { expected: String, got: Token },
    #[error("unknown column type '{0}'")]
    UnknownColType(String),
    #[error("unknown statement starting with {0:?}")]
    UnknownStatement(Token),
    #[error("invalid ON condition: expected table.column = table.column")]
    InvalidOnCondition,
    #[error("EXPLAIN supports SELECT, UPDATE and DELETE statements")]
    ExplainUnsupported,
    #[error("HAVING conditions must compare an aggregate, e.g. HAVING COUNT(*) > 1")]
    HavingNeedsAggregate,
    #[error("{0}(*) is not valid: only COUNT accepts *")]
    StarOnlyForCount(String),
}

pub type Result<T> = std::result::Result<T, ParseError>;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    pub fn parse(mut self) -> Result<Stmt> {
        let stmt = self.parse_stmt()?;
        if self.peek() == Some(&Token::Semicolon) {
            self.advance();
        }
        Ok(stmt)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_nth(&self, n: usize) -> Option<&Token> {
        self.tokens.get(self.pos + n)
    }

    fn advance(&mut self) -> Option<&Token> {
        let tok = self.tokens.get(self.pos);
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<()> {
        match self.advance() {
            Some(t) if t == expected => Ok(()),
            Some(t) => Err(ParseError::Expected {
                expected: format!("{:?}", expected),
                got: t.clone(),
            }),
            None => Err(ParseError::UnexpectedEof),
        }
    }

    fn expect_ident(&mut self) -> Result<String> {
        match self.advance() {
            Some(Token::Ident(s)) => Ok(s.clone()),
            Some(t) => Err(ParseError::Expected {
                expected: "identifier".to_string(),
                got: t.clone(),
            }),
            None => Err(ParseError::UnexpectedEof),
        }
    }

    fn parse_stmt(&mut self) -> Result<Stmt> {
        match self.peek().ok_or(ParseError::UnexpectedEof)? {
            Token::Create => self.parse_create(),
            Token::Insert => self.parse_insert(),
            Token::Select => self.parse_select(),
            Token::Update => self.parse_update(),
            Token::Delete => self.parse_delete(),
            Token::Explain => {
                self.advance();
                let inner = self.parse_stmt()?;
                match inner {
                    Stmt::Select(_) | Stmt::Update(_) | Stmt::Delete(_) => {
                        Ok(Stmt::Explain(Box::new(inner)))
                    }
                    _ => Err(ParseError::ExplainUnsupported),
                }
            }
            Token::Show => {
                self.advance();
                let what = self.expect_ident()?;
                if what.eq_ignore_ascii_case("tables") {
                    Ok(Stmt::ShowTables)
                } else {
                    Err(ParseError::Expected {
                        expected: "TABLES".to_string(),
                        got: Token::Ident(what),
                    })
                }
            }
            Token::Describe => {
                self.advance();
                Ok(Stmt::Describe(self.expect_ident()?))
            }
            Token::Begin => {
                self.advance();
                Ok(Stmt::Begin)
            }
            Token::Commit => {
                self.advance();
                Ok(Stmt::Commit)
            }
            Token::Rollback => {
                self.advance();
                Ok(Stmt::Rollback)
            }
            other => Err(ParseError::UnknownStatement(other.clone())),
        }
    }

    fn parse_create(&mut self) -> Result<Stmt> {
        self.expect(&Token::Create)?;
        if self.peek() == Some(&Token::Index) {
            return self.parse_create_index();
        }
        self.expect(&Token::Table)?;
        let table_name = self.expect_ident()?;
        self.expect(&Token::LParen)?;
        let mut columns = Vec::new();
        loop {
            let name = self.expect_ident()?;
            let type_ident = self.expect_ident()?;
            let col_type =
                ColType::from_str(&type_ident).ok_or(ParseError::UnknownColType(type_ident))?;
            columns.push(ColumnDef { name, col_type });
            match self.peek() {
                Some(Token::Comma) => {
                    self.advance();
                }
                _ => break,
            }
        }
        self.expect(&Token::RParen)?;
        Ok(Stmt::CreateTable(CreateTableStmt {
            table_name,
            columns,
        }))
    }

    fn parse_create_index(&mut self) -> Result<Stmt> {
        self.expect(&Token::Index)?;
        let index_name = self.expect_ident()?;
        self.expect(&Token::On)?;
        let table_name = self.expect_ident()?;
        self.expect(&Token::LParen)?;
        let column = self.expect_ident()?;
        self.expect(&Token::RParen)?;
        Ok(Stmt::CreateIndex(CreateIndexStmt {
            index_name,
            table_name,
            column,
        }))
    }

    fn parse_insert(&mut self) -> Result<Stmt> {
        self.expect(&Token::Insert)?;
        self.expect(&Token::Into)?;
        let table_name = self.expect_ident()?;
        self.expect(&Token::LParen)?;
        let mut columns = Vec::new();
        loop {
            columns.push(self.expect_ident()?);
            match self.peek() {
                Some(Token::Comma) => {
                    self.advance();
                }
                _ => break,
            }
        }
        self.expect(&Token::RParen)?;
        self.expect(&Token::Values)?;
        let mut rows = Vec::new();
        loop {
            self.expect(&Token::LParen)?;
            let mut row = Vec::new();
            loop {
                row.push(self.parse_value()?);
                match self.peek() {
                    Some(Token::Comma) => {
                        self.advance();
                    }
                    _ => break,
                }
            }
            self.expect(&Token::RParen)?;
            rows.push(row);
            match self.peek() {
                Some(Token::Comma) => {
                    self.advance();
                }
                _ => break,
            }
        }
        Ok(Stmt::Insert(InsertStmt {
            table_name,
            columns,
            rows,
        }))
    }

    fn parse_select(&mut self) -> Result<Stmt> {
        self.expect(&Token::Select)?;

        // Column list — supports star, bare names, table.column and aggregates
        let columns = if self.peek() == Some(&Token::Star) {
            self.advance();
            SelectColumns::Star
        } else {
            let mut items = Vec::new();
            loop {
                items.push(self.parse_select_item()?);
                match self.peek() {
                    Some(Token::Comma) => {
                        self.advance();
                    }
                    _ => break,
                }
            }
            if items.iter().any(|i| matches!(i, SelectItem::Aggregate(_))) {
                SelectColumns::Items(items)
            } else {
                SelectColumns::Named(
                    items
                        .into_iter()
                        .filter_map(|i| match i {
                            SelectItem::Column(c) => Some(c),
                            SelectItem::Aggregate(_) => None,
                        })
                        .collect(),
                )
            }
        };

        self.expect(&Token::From)?;
        let table_name = self.expect_ident()?;

        // JOIN clauses
        let mut joins = Vec::new();
        loop {
            let join_type = match self.peek() {
                Some(Token::Inner) => {
                    self.advance();
                    self.expect(&Token::Join)?;
                    JoinType::Inner
                }
                Some(Token::Left) => {
                    self.advance();
                    self.expect(&Token::Join)?;
                    JoinType::Left
                }
                Some(Token::Join) => {
                    self.advance();
                    JoinType::Inner
                }
                _ => break,
            };

            let join_table = self.expect_ident()?;
            self.expect(&Token::On)?;
            let on = self.parse_join_condition()?;
            joins.push(JoinClause {
                join_type,
                table_name: join_table,
                on,
            });
        }

        // WHERE
        let where_clause = if self.peek() == Some(&Token::Where) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };

        // GROUP BY
        let mut group_by = Vec::new();
        if self.peek() == Some(&Token::Group) {
            self.advance();
            self.expect(&Token::By)?;
            loop {
                group_by.push(self.parse_column_ref()?);
                match self.peek() {
                    Some(Token::Comma) => {
                        self.advance();
                    }
                    _ => break,
                }
            }
        }

        // HAVING
        let having = if self.peek() == Some(&Token::Having) {
            self.advance();
            Some(self.parse_having_or()?)
        } else {
            None
        };

        // ORDER BY
        let mut order_by = Vec::new();
        if self.peek() == Some(&Token::Order) {
            self.advance();
            self.expect(&Token::By)?;
            loop {
                // An aggregate (`ORDER BY COUNT(*) DESC`) is stored by its header name.
                let column = match self.parse_select_item()? {
                    SelectItem::Aggregate(agg) => agg.display_name(),
                    SelectItem::Column(c) => c.column,
                };
                let direction = match self.peek() {
                    Some(Token::Asc) => {
                        self.advance();
                        OrderDirection::Asc
                    }
                    Some(Token::Desc) => {
                        self.advance();
                        OrderDirection::Desc
                    }
                    _ => OrderDirection::Asc,
                };
                order_by.push(OrderByClause { column, direction });
                match self.peek() {
                    Some(Token::Comma) => {
                        self.advance();
                    }
                    _ => break,
                }
            }
        }

        // LIMIT
        let limit = if self.peek() == Some(&Token::Limit) {
            self.advance();
            match self.advance() {
                Some(Token::IntLiteral(n)) => Some(*n as usize),
                Some(t) => {
                    return Err(ParseError::Expected {
                        expected: "integer".to_string(),
                        got: t.clone(),
                    })
                }
                None => return Err(ParseError::UnexpectedEof),
            }
        } else {
            None
        };

        Ok(Stmt::Select(SelectStmt {
            table_name,
            columns,
            joins,
            where_clause,
            group_by,
            having,
            order_by,
            limit,
        }))
    }

    /// Parse one select-list entry: an aggregate such as `COUNT(*)` / `SUM(age)`,
    /// or a plain (optionally qualified) column.
    ///
    /// Aggregate names are not keywords: `count` is only treated as a function
    /// when it is directly followed by `(`, so columns named `count` or `max` still work.
    fn parse_select_item(&mut self) -> Result<SelectItem> {
        if let (Some(Token::Ident(name)), Some(Token::LParen)) = (self.peek(), self.peek_nth(1)) {
            if let Some(func) = AggFunc::from_name(name) {
                self.advance(); // function name
                self.advance(); // (
                let arg = if self.peek() == Some(&Token::Star) {
                    if func != AggFunc::Count {
                        return Err(ParseError::StarOnlyForCount(func.name().to_string()));
                    }
                    self.advance();
                    AggArg::Star
                } else {
                    AggArg::Column(self.parse_column_ref()?)
                };
                self.expect(&Token::RParen)?;
                return Ok(SelectItem::Aggregate(AggregateExpr { func, arg }));
            }
        }
        Ok(SelectItem::Column(self.parse_column_ref()?))
    }

    /// Parse a column reference: either `col` or `table.col`
    fn parse_column_ref(&mut self) -> Result<ColumnRef> {
        let first = self.expect_ident()?;
        if self.peek() == Some(&Token::Dot) {
            self.advance();
            let col = self.expect_ident()?;
            Ok(ColumnRef::qualified(&first, &col))
        } else {
            Ok(ColumnRef::unqualified(&first))
        }
    }

    /// Parse ON left_table.left_col = right_table.right_col
    fn parse_join_condition(&mut self) -> Result<JoinCondition> {
        let left_table = self.expect_ident()?;
        self.expect(&Token::Dot)?;
        let left_col = self.expect_ident()?;
        self.expect(&Token::Eq)?;
        let right_table = self.expect_ident()?;
        self.expect(&Token::Dot)?;
        let right_col = self.expect_ident()?;
        Ok(JoinCondition {
            left_table,
            left_col,
            right_table,
            right_col,
        })
    }

    fn parse_update(&mut self) -> Result<Stmt> {
        self.expect(&Token::Update)?;
        let table_name = self.expect_ident()?;
        self.expect(&Token::Set)?;
        let mut assignments = Vec::new();
        loop {
            let column = self.expect_ident()?;
            self.expect(&Token::Eq)?;
            let value = self.parse_value()?;
            assignments.push(Assignment { column, value });
            match self.peek() {
                Some(Token::Comma) => {
                    self.advance();
                }
                _ => break,
            }
        }
        let where_clause = if self.peek() == Some(&Token::Where) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Stmt::Update(UpdateStmt {
            table_name,
            assignments,
            where_clause,
        }))
    }

    fn parse_delete(&mut self) -> Result<Stmt> {
        self.expect(&Token::Delete)?;
        self.expect(&Token::From)?;
        let table_name = self.expect_ident()?;
        let where_clause = if self.peek() == Some(&Token::Where) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Stmt::Delete(DeleteStmt {
            table_name,
            where_clause,
        }))
    }

    // HAVING uses standard precedence: AND binds tighter than OR.
    fn parse_having_or(&mut self) -> Result<HavingExpr> {
        let mut left = self.parse_having_and()?;
        while self.peek() == Some(&Token::Or) {
            self.advance();
            let right = self.parse_having_and()?;
            left = HavingExpr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_having_and(&mut self) -> Result<HavingExpr> {
        let mut left = self.parse_having_comparison()?;
        while self.peek() == Some(&Token::And) {
            self.advance();
            let right = self.parse_having_comparison()?;
            left = HavingExpr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_having_comparison(&mut self) -> Result<HavingExpr> {
        match self.parse_select_item()? {
            SelectItem::Aggregate(agg) => {
                let op = self.parse_cmp_op()?;
                let value = self.parse_value()?;
                Ok(HavingExpr::Comparison { agg, op, value })
            }
            SelectItem::Column(_) => Err(ParseError::HavingNeedsAggregate),
        }
    }

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_and()
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_or()?;
        while self.peek() == Some(&Token::And) {
            self.advance();
            let right = self.parse_or()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_comparison()?;
        while self.peek() == Some(&Token::Or) {
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr> {
        let column = self.expect_ident()?;
        let op = self.parse_cmp_op()?;
        let value = self.parse_value()?;
        Ok(Expr::Comparison { column, op, value })
    }

    fn parse_cmp_op(&mut self) -> Result<CmpOp> {
        match self.advance() {
            Some(Token::Eq) => Ok(CmpOp::Eq),
            Some(Token::NotEq) => Ok(CmpOp::NotEq),
            Some(Token::Lt) => Ok(CmpOp::Lt),
            Some(Token::Lte) => Ok(CmpOp::Lte),
            Some(Token::Gt) => Ok(CmpOp::Gt),
            Some(Token::Gte) => Ok(CmpOp::Gte),
            Some(t) => Err(ParseError::Expected {
                expected: "comparison operator".to_string(),
                got: t.clone(),
            }),
            None => Err(ParseError::UnexpectedEof),
        }
    }

    fn parse_value(&mut self) -> Result<Value> {
        match self.advance() {
            Some(Token::IntLiteral(n)) => Ok(Value::Integer(*n)),
            Some(Token::TextLiteral(s)) => Ok(Value::Text(s.clone())),
            Some(Token::Null) => Ok(Value::Null),
            Some(t) => Err(ParseError::Expected {
                expected: "value".to_string(),
                got: t.clone(),
            }),
            None => Err(ParseError::UnexpectedEof),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::lexer::Lexer;

    fn parse(sql: &str) -> Stmt {
        let tokens = Lexer::new(sql).tokenize().unwrap();
        Parser::new(tokens).parse().unwrap()
    }

    #[test]
    fn parse_create_table() {
        let stmt = parse("CREATE TABLE users (id INTEGER, name TEXT)");
        match stmt {
            Stmt::CreateTable(s) => assert_eq!(s.columns.len(), 2),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_insert_single_row() {
        let stmt = parse("INSERT INTO users (id, name) VALUES (1, 'Alice')");
        match stmt {
            Stmt::Insert(s) => {
                assert_eq!(s.rows[0][0], Value::Integer(1));
                assert_eq!(s.rows[0][1], Value::Text("Alice".to_string()));
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_insert_multi_row() {
        let stmt = parse("INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob')");
        match stmt {
            Stmt::Insert(s) => assert_eq!(s.rows.len(), 2),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_select_star() {
        let stmt = parse("SELECT * FROM users");
        match stmt {
            Stmt::Select(s) => {
                assert!(matches!(s.columns, SelectColumns::Star));
                assert!(s.joins.is_empty());
                assert!(s.where_clause.is_none());
                assert!(s.order_by.is_empty());
                assert!(s.limit.is_none());
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_select_columns() {
        let stmt = parse("SELECT id, name FROM users");
        match stmt {
            Stmt::Select(s) => assert!(matches!(s.columns, SelectColumns::Named(_))),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_select_where() {
        let stmt = parse("SELECT * FROM users WHERE age > 25");
        match stmt {
            Stmt::Select(s) => assert!(s.where_clause.is_some()),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_where_and() {
        let stmt = parse("SELECT * FROM users WHERE age > 18 AND name = 'Alice'");
        match stmt {
            Stmt::Select(s) => assert!(matches!(s.where_clause, Some(Expr::And(_, _)))),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_where_or() {
        let stmt = parse("SELECT * FROM users WHERE age < 10 OR age > 90");
        match stmt {
            Stmt::Select(s) => assert!(matches!(s.where_clause, Some(Expr::Or(_, _)))),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_order_by() {
        let stmt = parse("SELECT * FROM users ORDER BY age DESC");
        match stmt {
            Stmt::Select(s) => {
                assert_eq!(s.order_by.len(), 1);
                assert_eq!(s.order_by[0].direction, OrderDirection::Desc);
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_order_by_multi() {
        let stmt = parse("SELECT * FROM users ORDER BY age ASC, name DESC");
        match stmt {
            Stmt::Select(s) => assert_eq!(s.order_by.len(), 2),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_limit() {
        let stmt = parse("SELECT * FROM users LIMIT 5");
        match stmt {
            Stmt::Select(s) => assert_eq!(s.limit, Some(5)),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_order_by_and_limit() {
        let stmt = parse("SELECT * FROM users ORDER BY age DESC LIMIT 3");
        match stmt {
            Stmt::Select(s) => {
                assert_eq!(s.order_by[0].direction, OrderDirection::Desc);
                assert_eq!(s.limit, Some(3));
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_update() {
        let stmt = parse("UPDATE users SET age = 31 WHERE id = 1");
        match stmt {
            Stmt::Update(s) => {
                assert_eq!(s.assignments.len(), 1);
                assert!(s.where_clause.is_some());
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_update_multi_set() {
        let stmt = parse("UPDATE users SET age = 31, name = 'Bob' WHERE id = 1");
        match stmt {
            Stmt::Update(s) => assert_eq!(s.assignments.len(), 2),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_delete() {
        let stmt = parse("DELETE FROM users WHERE id = 1");
        match stmt {
            Stmt::Delete(s) => assert!(s.where_clause.is_some()),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_delete_no_where() {
        let stmt = parse("DELETE FROM users");
        match stmt {
            Stmt::Delete(s) => assert!(s.where_clause.is_none()),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_inner_join() {
        let stmt = parse(
            "SELECT users.name, orders.amount FROM users INNER JOIN orders ON users.id = orders.user_id"
        );
        match stmt {
            Stmt::Select(s) => {
                assert_eq!(s.joins.len(), 1);
                assert_eq!(s.joins[0].join_type, JoinType::Inner);
                assert_eq!(s.joins[0].table_name, "orders");
                assert_eq!(s.joins[0].on.left_col, "id");
                assert_eq!(s.joins[0].on.right_col, "user_id");
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_left_join() {
        let stmt = parse(
            "SELECT users.name, orders.amount FROM users LEFT JOIN orders ON users.id = orders.user_id"
        );
        match stmt {
            Stmt::Select(s) => {
                assert_eq!(s.joins[0].join_type, JoinType::Left);
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_qualified_columns() {
        let stmt = parse("SELECT users.name, orders.amount FROM users");
        match stmt {
            Stmt::Select(s) => {
                if let SelectColumns::Named(cols) = s.columns {
                    assert_eq!(cols[0].table, Some("users".to_string()));
                    assert_eq!(cols[0].column, "name");
                    assert_eq!(cols[1].table, Some("orders".to_string()));
                }
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_count_star() {
        let stmt = parse("SELECT COUNT(*) FROM users");
        match stmt {
            Stmt::Select(s) => match s.columns {
                SelectColumns::Items(items) => {
                    assert_eq!(items.len(), 1);
                    match &items[0] {
                        SelectItem::Aggregate(a) => {
                            assert_eq!(a.func, AggFunc::Count);
                            assert!(matches!(a.arg, AggArg::Star));
                            assert_eq!(a.display_name(), "COUNT(*)");
                        }
                        _ => panic!("expected aggregate"),
                    }
                }
                _ => panic!("expected Items"),
            },
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_multiple_aggregates_case_insensitive() {
        let stmt =
            parse("SELECT count(*), Sum(age), AVG(users.age), min(age), MAX(age) FROM users");
        match stmt {
            Stmt::Select(s) => match s.columns {
                SelectColumns::Items(items) => assert_eq!(items.len(), 5),
                _ => panic!("expected Items"),
            },
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_aggregate_with_where() {
        let stmt = parse("SELECT SUM(amount) FROM orders WHERE amount > 10");
        match stmt {
            Stmt::Select(s) => assert!(s.where_clause.is_some()),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn column_named_like_aggregate_is_still_a_column() {
        let stmt = parse("SELECT count, max FROM stats");
        match stmt {
            Stmt::Select(s) => assert!(matches!(s.columns, SelectColumns::Named(_))),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn sum_star_is_rejected() {
        let tokens = Lexer::new("SELECT SUM(*) FROM t").tokenize().unwrap();
        assert!(matches!(
            Parser::new(tokens).parse(),
            Err(ParseError::StarOnlyForCount(_))
        ));
    }

    #[test]
    fn parse_group_by_single_and_multiple() {
        match parse("SELECT age, COUNT(*) FROM users GROUP BY age") {
            Stmt::Select(s) => assert_eq!(s.group_by.len(), 1),
            _ => panic!("wrong stmt"),
        }
        match parse("SELECT a, b, COUNT(*) FROM t GROUP BY a, t.b") {
            Stmt::Select(s) => {
                assert_eq!(s.group_by.len(), 2);
                assert_eq!(s.group_by[1].table, Some("t".to_string()));
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_having() {
        match parse("SELECT age FROM users GROUP BY age HAVING COUNT(*) > 1") {
            Stmt::Select(s) => assert!(matches!(
                s.having,
                Some(HavingExpr::Comparison { op: CmpOp::Gt, .. })
            )),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_having_precedence_and_binds_tighter_than_or() {
        let sql = "SELECT a FROM t GROUP BY a HAVING COUNT(*) > 1 OR SUM(b) > 5 AND MAX(b) < 9";
        match parse(sql) {
            Stmt::Select(s) => match s.having {
                Some(HavingExpr::Or(_, right)) => {
                    assert!(matches!(*right, HavingExpr::And(_, _)))
                }
                other => panic!("expected OR at the top, got {:?}", other),
            },
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn having_requires_an_aggregate() {
        let tokens = Lexer::new("SELECT a FROM t GROUP BY a HAVING a > 1")
            .tokenize()
            .unwrap();
        assert!(matches!(
            Parser::new(tokens).parse(),
            Err(ParseError::HavingNeedsAggregate)
        ));
    }

    #[test]
    fn parse_order_by_aggregate() {
        let sql = "SELECT a, COUNT(*) FROM t GROUP BY a ORDER BY COUNT(*) DESC, a ASC LIMIT 3";
        match parse(sql) {
            Stmt::Select(s) => {
                assert_eq!(s.order_by[0].column, "COUNT(*)");
                assert_eq!(s.order_by[0].direction, OrderDirection::Desc);
                assert_eq!(s.order_by[1].column, "a");
                assert_eq!(s.limit, Some(3));
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn parse_create_index() {
        match parse("CREATE INDEX idx_age ON users (age)") {
            Stmt::CreateIndex(s) => {
                assert_eq!(s.index_name, "idx_age");
                assert_eq!(s.table_name, "users");
                assert_eq!(s.column, "age");
            }
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn create_index_requires_on_and_column() {
        for sql in [
            "CREATE INDEX idx users (age)",
            "CREATE INDEX idx ON users age",
            "CREATE INDEX ON users (age)",
        ] {
            let tokens = Lexer::new(sql).tokenize().unwrap();
            assert!(
                Parser::new(tokens).parse().is_err(),
                "{sql} should not parse"
            );
        }
    }

    #[test]
    fn parse_transaction_statements() {
        assert!(matches!(parse("BEGIN"), Stmt::Begin));
        assert!(matches!(parse("commit;"), Stmt::Commit));
        assert!(matches!(parse("ROLLBACK"), Stmt::Rollback));
    }

    #[test]
    fn parse_explain_show_describe() {
        match parse("EXPLAIN SELECT * FROM t WHERE a = 1") {
            Stmt::Explain(inner) => assert!(matches!(*inner, Stmt::Select(_))),
            _ => panic!("wrong stmt"),
        }
        assert!(matches!(parse("EXPLAIN DELETE FROM t"), Stmt::Explain(_)));
        assert!(matches!(parse("show tables"), Stmt::ShowTables));
        match parse("DESCRIBE users;") {
            Stmt::Describe(t) => assert_eq!(t, "users"),
            _ => panic!("wrong stmt"),
        }
    }

    #[test]
    fn explain_rejects_unsupported_statements() {
        let tokens = Lexer::new("EXPLAIN INSERT INTO t (a) VALUES (1)")
            .tokenize()
            .unwrap();
        assert!(matches!(
            Parser::new(tokens).parse(),
            Err(ParseError::ExplainUnsupported)
        ));
        let tokens = Lexer::new("SHOW columns").tokenize().unwrap();
        assert!(Parser::new(tokens).parse().is_err());
    }
}
