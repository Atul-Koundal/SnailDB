use super::token::Token;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LexerError {
    #[error("unexpected character '{0}' at position {1}")]
    UnexpectedChar(char, usize),
    #[error("unterminated string literal")]
    UnterminatedString,
}

pub type Result<T> = std::result::Result<T, LexerError>;

pub struct Lexer<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Lexer { input, pos: 0 }
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace();
            if self.pos >= self.input.len() {
                break;
            }
            let tok = self.next_token()?;
            tokens.push(tok);
        }
        Ok(tokens)
    }

    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek();
        if let Some(c) = ch {
            self.pos += c.len_utf8();
        }
        ch
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_ascii_whitespace()) {
            self.advance();
        }
    }

    fn next_token(&mut self) -> Result<Token> {
        let ch = self.peek().unwrap();

        if ch == '\'' {
            return self.read_string();
        }
        if ch.is_ascii_digit() || (ch == '-' && self.next_is_digit()) {
            return Ok(self.read_number());
        }
        if ch.is_alphabetic() || ch == '_' {
            return Ok(self.read_ident_or_keyword());
        }
        if ch == '!' {
            self.advance();
            if self.peek() == Some('=') {
                self.advance();
                return Ok(Token::NotEq);
            }
            return Err(LexerError::UnexpectedChar('!', self.pos));
        }
        if ch == '<' {
            self.advance();
            if self.peek() == Some('=') { self.advance(); return Ok(Token::Lte); }
            return Ok(Token::Lt);
        }
        if ch == '>' {
            self.advance();
            if self.peek() == Some('=') { self.advance(); return Ok(Token::Gte); }
            return Ok(Token::Gt);
        }

        self.advance();
        match ch {
            '=' => Ok(Token::Eq),
            '(' => Ok(Token::LParen),
            ')' => Ok(Token::RParen),
            ',' => Ok(Token::Comma),
            ';' => Ok(Token::Semicolon),
            '*' => Ok(Token::Star),
            '.' => Ok(Token::Dot),
            other => Err(LexerError::UnexpectedChar(other, self.pos)),
        }
    }

    fn next_is_digit(&self) -> bool {
        let mut chars = self.input[self.pos..].chars();
        chars.next();
        matches!(chars.next(), Some(c) if c.is_ascii_digit())
    }

    fn read_number(&mut self) -> Token {
        let start = self.pos;
        if self.peek() == Some('-') { self.advance(); }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.advance();
        }
        let n: i64 = self.input[start..self.pos].parse().unwrap();
        Token::IntLiteral(n)
    }

    fn read_string(&mut self) -> Result<Token> {
        self.advance();
        let mut s = String::new();
        loop {
            match self.advance() {
                None       => return Err(LexerError::UnterminatedString),
                Some('\'') => break,
                Some(c)    => s.push(c),
            }
        }
        Ok(Token::TextLiteral(s))
    }

    fn read_ident_or_keyword(&mut self) -> Token {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_') {
            self.advance();
        }
        let word = &self.input[start..self.pos];
        match word.to_uppercase().as_str() {
            "CREATE" => Token::Create,
            "TABLE"  => Token::Table,
            "INSERT" => Token::Insert,
            "INTO"   => Token::Into,
            "VALUES" => Token::Values,
            "SELECT" => Token::Select,
            "FROM"   => Token::From,
            "WHERE"  => Token::Where,
            "AND"    => Token::And,
            "OR"     => Token::Or,
            "NOT"    => Token::Not,
            "NULL"   => Token::Null,
            "UPDATE" => Token::Update,
            "SET"    => Token::Set,
            "DELETE" => Token::Delete,
            "ORDER"  => Token::Order,
            "BY"     => Token::By,
            "ASC"    => Token::Asc,
            "DESC"   => Token::Desc,
            "LIMIT"  => Token::Limit,
            "JOIN"   => Token::Join,
            "INNER"  => Token::Inner,
            "LEFT"   => Token::Left,
            "RIGHT"  => Token::Right,
            "ON"     => Token::On,
            _        => Token::Ident(word.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(s: &str) -> Vec<Token> {
        Lexer::new(s).tokenize().unwrap()
    }

    #[test]
    fn keywords_and_idents() {
        let tokens = lex("SELECT name FROM users");
        assert_eq!(tokens, vec![
            Token::Select,
            Token::Ident("name".to_string()),
            Token::From,
            Token::Ident("users".to_string()),
        ]);
    }

    #[test]
    fn string_and_int_literals() {
        let tokens = lex("INSERT INTO t VALUES (1, 'alice')");
        assert!(tokens.contains(&Token::IntLiteral(1)));
        assert!(tokens.contains(&Token::TextLiteral("alice".to_string())));
    }

    #[test]
    fn operators() {
        let tokens = lex("!= <= >= < > =");
        assert_eq!(tokens, vec![
            Token::NotEq, Token::Lte, Token::Gte,
            Token::Lt, Token::Gt, Token::Eq,
        ]);
    }

    #[test]
    fn star_and_punctuation() {
        let tokens = lex("SELECT * FROM t;");
        assert_eq!(tokens[1], Token::Star);
        assert_eq!(tokens[4], Token::Semicolon);
    }

    #[test]
    fn new_keywords() {
        let tokens = lex("UPDATE users SET age = 31 WHERE id = 1");
        assert_eq!(tokens[0], Token::Update);
        assert_eq!(tokens[2], Token::Set);

        let tokens = lex("DELETE FROM users WHERE id = 1");
        assert_eq!(tokens[0], Token::Delete);

        let tokens = lex("SELECT * FROM users ORDER BY age DESC LIMIT 5");
        assert!(tokens.contains(&Token::Order));
        assert!(tokens.contains(&Token::Desc));
        assert!(tokens.contains(&Token::Limit));
    }

    #[test]
    fn join_keywords_and_dot() {
        let tokens = lex("SELECT users.name FROM users INNER JOIN orders ON users.id = orders.user_id");
        assert!(tokens.contains(&Token::Inner));
        assert!(tokens.contains(&Token::Join));
        assert!(tokens.contains(&Token::On));
        assert!(tokens.contains(&Token::Dot));
    }
}