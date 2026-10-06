use crate::lexer::Span;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    I32, I64, U32, U64, F32, F64, Bool,
    Array(Box<Type>, Option<usize>),
    Custom(String),
    Pointer(Box<Type>),
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        typ: Option<Type>,
        value: Box<Expr>,
        span: Span,
    },
    FnDef {
        visibility: Visibility,
        name: String,
        params: Vec<(String, Type)>,
        return_type: Option<Type>,
        body: Block,
        span: Span,
    },
    FnDecl {
        visibility: Visibility,
        name: String,
        params: Vec<(String, Type)>,
        return_type: Option<Type>,
        span: Span,
    },
    Include {
        path: String,
        span: Span,
    },
    EnumDef {
        name: String,
        variants: Vec<String>,
        span: Span,
    },
    Expr(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(Literal),
    Variable(String),
    BinaryOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    ArrayInit {
        elements: Vec<Expr>,
        span: Span,
    },
    /// `tablica[indeks]` - odczyt (i, jako l-value po prawej stronie `$~`,
    /// zapis) elementu tablicy. `array` musi dziś być bezpośrednio zmienną
    /// (patrz ograniczenia w lowering.rs) - indeksowanie wyrażeń złożonych
    /// nie jest jeszcze obsługiwane.
    Index {
        array: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    If {
        condition: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Block>,
            span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Option<Span> {
        match self {
            Expr::BinaryOp { span, .. } => Some(*span),
            Expr::Call { span, .. } => Some(*span),
            Expr::ArrayInit { span, .. } => Some(*span),
            Expr::Index { span, .. } => Some(*span),
            Expr::If { span, .. } => Some(*span),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(String),
    Float(String),
    String(String),
    Bool(bool),
    None,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Neq, Lt, Gt, Le, Ge,
    Spaceship,
    HashEqEq,
    PipeEq,
    TildeEq,
    At, AtSlash, AtEq,
    MinusEq,
    SyncAssign,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}
