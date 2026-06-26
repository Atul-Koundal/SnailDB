use crate::catalog::ColType;

#[derive(Debug, Clone)]
pub enum Stmt {
    CreateTable(CreateTableStmt),
    Insert(InsertStmt),
    Select(SelectStmt),
    Update(UpdateStmt),
    Delete(DeleteStmt),
}

// ── CREATE TABLE ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CreateTableStmt {
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
}

#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub col_type: ColType,
}

// ── INSERT ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct InsertStmt {
    pub table_name: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

// ── SELECT ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SelectStmt {
    pub table_name: String,
    pub columns: SelectColumns,
    pub where_clause: Option<Expr>,
    pub order_by: Vec<OrderByClause>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum SelectColumns {
    Star,
    Named(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct OrderByClause {
    pub column: String,
    pub direction: OrderDirection,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrderDirection {
    Asc,
    Desc,
}

// ── UPDATE ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UpdateStmt {
    pub table_name: String,
    pub assignments: Vec<Assignment>,
    pub where_clause: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub column: String,
    pub value: Value,
}

// ── DELETE ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DeleteStmt {
    pub table_name: String,
    pub where_clause: Option<Expr>,
}

// ── Expressions (WHERE) ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Expr {
    Comparison {
        column: String,
        op: CmpOp,
        value: Value,
    },
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CmpOp {
    Eq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
}

// ── Values ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Integer(i64),
    Text(String),
    Null,
}