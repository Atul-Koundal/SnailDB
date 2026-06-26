#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // ── Keywords ──────────────────────────────────────────────────────
    Create,
    Table,
    Insert,
    Into,
    Values,
    Select,
    From,
    Where,
    And,
    Or,
    Not,
    Null,
    Update,
    Set,
    Delete,
    Order,
    By,
    Asc,
    Desc,
    Limit,
    Join,
    Inner,
    Left,
    Right,
    On,

    // ── Identifiers & literals ────────────────────────────────────────
    Ident(String),
    IntLiteral(i64),
    TextLiteral(String),

    // ── Punctuation ───────────────────────────────────────────────────
    LParen,
    RParen,
    Comma,
    Semicolon,
    Star,
    Dot,

    // ── Operators ─────────────────────────────────────────────────────
    Eq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
}