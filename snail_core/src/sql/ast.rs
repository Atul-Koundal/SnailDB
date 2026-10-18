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
    pub joins: Vec<JoinClause>,
    pub where_clause: Option<Expr>,
    pub group_by: Vec<ColumnRef>,
    pub having: Option<HavingExpr>,
    pub order_by: Vec<OrderByClause>,
    pub limit: Option<usize>,
}

/// Condition of a HAVING clause: comparisons of aggregates combined with AND / OR.
#[derive(Debug, Clone)]
pub enum HavingExpr {
    Comparison {
        agg: AggregateExpr,
        op: CmpOp,
        value: Value,
    },
    And(Box<HavingExpr>, Box<HavingExpr>),
    Or(Box<HavingExpr>, Box<HavingExpr>),
}

#[derive(Debug, Clone)]
pub enum SelectColumns {
    Star,
    /// Plain column references only (`SELECT id, users.name ...`).
    Named(Vec<ColumnRef>),
    /// Select list containing at least one aggregate (`SELECT COUNT(*), age ...`).
    Items(Vec<SelectItem>),
}

/// One entry of a select list that contains aggregates.
#[derive(Debug, Clone)]
pub enum SelectItem {
    Column(ColumnRef),
    Aggregate(AggregateExpr),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl AggFunc {
    /// Case-insensitive lookup, e.g. `"count"` -> `Some(AggFunc::Count)`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_uppercase().as_str() {
            "COUNT" => Some(AggFunc::Count),
            "SUM" => Some(AggFunc::Sum),
            "AVG" => Some(AggFunc::Avg),
            "MIN" => Some(AggFunc::Min),
            "MAX" => Some(AggFunc::Max),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            AggFunc::Count => "COUNT",
            AggFunc::Sum => "SUM",
            AggFunc::Avg => "AVG",
            AggFunc::Min => "MIN",
            AggFunc::Max => "MAX",
        }
    }
}

/// Argument of an aggregate: `*` (only valid for COUNT) or a column.
#[derive(Debug, Clone)]
pub enum AggArg {
    Star,
    Column(ColumnRef),
}

#[derive(Debug, Clone)]
pub struct AggregateExpr {
    pub func: AggFunc,
    pub arg: AggArg,
}

impl AggregateExpr {
    /// Output column header, e.g. `COUNT(*)` or `SUM(orders.amount)`.
    pub fn display_name(&self) -> String {
        match &self.arg {
            AggArg::Star => format!("{}(*)", self.func.name()),
            AggArg::Column(c) => format!("{}({})", self.func.name(), c.display_name()),
        }
    }
}

/// A column reference — either `col` or `table.col`
#[derive(Debug, Clone)]
pub struct ColumnRef {
    pub table: Option<String>,
    pub column: String,
}

impl ColumnRef {
    pub fn unqualified(column: &str) -> Self {
        ColumnRef {
            table: None,
            column: column.to_string(),
        }
    }
    pub fn qualified(table: &str, column: &str) -> Self {
        ColumnRef {
            table: Some(table.to_string()),
            column: column.to_string(),
        }
    }
    /// Display name used as the output column header
    pub fn display_name(&self) -> String {
        match &self.table {
            Some(t) => format!("{}.{}", t, self.column),
            None => self.column.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct JoinClause {
    pub join_type: JoinType,
    pub table_name: String,
    pub on: JoinCondition,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinType {
    Inner,
    Left,
}

/// ON left_table.left_col = right_table.right_col
#[derive(Debug, Clone)]
pub struct JoinCondition {
    pub left_table: String,
    pub left_col: String,
    pub right_table: String,
    pub right_col: String,
}

#[derive(Debug, Clone)]
pub struct OrderByClause {
    /// A column name, or for grouped queries the header of an aggregate such as `COUNT(*)`.
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
