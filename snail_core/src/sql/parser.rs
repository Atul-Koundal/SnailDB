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
        if self.peek() == Some(&Token::Semicolon) { self.advance(); }
        Ok(stmt)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
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
            other => Err(ParseError::UnknownStatement(other.clone())),
        }
    }

    fn parse_create(&mut self) -> Result<Stmt> {
        self.expect(&Token::Create)?;
        self.expect(&Token::Table)?;
        let table_name = self.expect_ident()?;
        self.expect(&Token::LParen)?;
        let mut columns = Vec::new();
        loop {
            let name = self.expect_ident()?;
            let type_ident = self.expect_ident()?;
            let col_type = ColType::from_str(&type_ident)
                .ok_or_else(|| ParseError::UnknownColType(type_ident))?;
            columns.push(ColumnDef { name, col_type });
            match self.peek() {
                Some(Token::Comma) => { self.advance(); }
                _ => break,
            }
        }
        self.expect(&Token::RParen)?;
        Ok(Stmt::CreateTable(CreateTableStmt { table_name, columns }))
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
                Some(Token::Comma) => { self.advance(); }
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
                    Some(Token::Comma) => { self.advance(); }
                    _ => break,
                }
            }
            self.expect(&Token::RParen)?;
            rows.push(row);
            match self.peek() {
                Some(Token::Comma) => { self.advance(); }
                _ => break,
            }
        }
        Ok(Stmt::Insert(InsertStmt { table_name, columns, rows }))
    }

    fn parse_select(&mut self) -> Result<Stmt> {
        self.expect(&Token::Select)?;

        // Column list — supports star, bare names, and table.column
        let columns = if self.peek() == Some(&Token::Star) {
            self.advance();
            SelectColumns::Star
        } else {
            let mut cols = Vec::new();
            loop {
                let col_ref = self.parse_column_ref()?;
                cols.push(col_ref);
                match self.peek() {
                    Some(Token::Comma) => { self.advance(); }
                    _ => break,
                }
            }
            SelectColumns::Named(cols)
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
            joins.push(JoinClause { join_type, table_name: join_table, on });
        }

        // WHERE
        let where_clause = if self.peek() == Some(&Token::Where) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };

        // ORDER BY
        let mut order_by = Vec::new();
        if self.peek() == Some(&Token::Order) {
            self.advance();
            self.expect(&Token::By)?;
            loop {
                let column = self.expect_ident()?;
                let direction = match self.peek() {
                    Some(Token::Asc)  => { self.advance(); OrderDirection::Asc  }
                    Some(Token::Desc) => { self.advance(); OrderDirection::Desc }
                    _ => OrderDirection::Asc,
                };
                order_by.push(OrderByClause { column, direction });
                match self.peek() {
                    Some(Token::Comma) => { self.advance(); }
                    _ => break,
                }
            }
        }

        // LIMIT
        let limit = if self.peek() == Some(&Token::Limit) {
            self.advance();
            match self.advance() {
                Some(Token::IntLiteral(n)) => Some(*n as usize),
                Some(t) => return Err(ParseError::Expected {
                    expected: "integer".to_string(),
                    got: t.clone(),
                }),
                None => return Err(ParseError::UnexpectedEof),
            }
        } else {
            None
        };

        Ok(Stmt::Select(SelectStmt {
            table_name, columns, joins, where_clause, order_by, limit,
        }))
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
        Ok(JoinCondition { left_table, left_col, right_table, right_col })
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
                Some(Token::Comma) => { self.advance(); }
                _ => break,
            }
        }
        let where_clause = if self.peek() == Some(&Token::Where) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Stmt::Update(UpdateStmt { table_name, assignments, where_clause }))
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
        Ok(Stmt::Delete(DeleteStmt { table_name, where_clause }))
    }

    fn parse_expr(&mut self) -> Result<Expr> { self.parse_and() }

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
            Some(Token::Eq)    => Ok(CmpOp::Eq),
            Some(Token::NotEq) => Ok(CmpOp::NotEq),
            Some(Token::Lt)    => Ok(CmpOp::Lt),
            Some(Token::Lte)   => Ok(CmpOp::Lte),
            Some(Token::Gt)    => Ok(CmpOp::Gt),
            Some(Token::Gte)   => Ok(CmpOp::Gte),
            Some(t) => Err(ParseError::Expected {
                expected: "comparison operator".to_string(),
                got: t.clone(),
            }),
            None => Err(ParseError::UnexpectedEof),
        }
    }

    fn parse_value(&mut self) -> Result<Value> {
        match self.advance() {
            Some(Token::IntLiteral(n))  => Ok(Value::Integer(*n)),
            Some(Token::TextLiteral(s)) => Ok(Value::Text(s.clone())),
            Some(Token::Null)           => Ok(Value::Null),
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
}