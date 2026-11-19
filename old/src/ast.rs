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
        is_mut: bool,
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
    /// Deklaracja funkcji zewnętrznej (FFI), np. `ext 'c' fn puts(s: String) -> i32;`.
    ExternDecl {
        lang: String,
        name: String,
        params: Vec<(String, Type)>,
        return_type: Option<Type>,
        span: Span,
    },
    /// Dyrektywa linkera, np. `link "-lm";` — dodatkowe flagi dla `cc`/`gcc`.
    Link {
        flags: Vec<String>,
        span: Span,
    },
    EnumDef {
        name: String,
        variants: Vec<String>,
        span: Span,
    },
    Expr(Box<Expr>),
    Return {
        value: Option<Box<Expr>>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal { value: Literal, typ: Type },
    Variable { name: String, typ: Type },
    BinaryOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
        typ: Type,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        typ: Type,
        span: Span,
    },
    /// Wywołanie funkcji zewnętrznej (FFI), np. `ext::'c'::puts("hi")`.
    ExternCall {
        lang: String,
        name: String,
        args: Vec<Expr>,
        typ: Type,
        span: Span,
    },
    ArrayInit {
        elements: Vec<Expr>,
        typ: Type,
        span: Span,
    },
    /// `tablica[indeks]` - odczyt (i, jako l-value po prawej stronie `$~`,
    /// zapis) elementu tablicy.
    Index {
        array: Box<Expr>,
        index: Box<Expr>,
        typ: Type,
        span: Span,
    },
    If {
        condition: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Block>,
        typ: Type,
        span: Span,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
        typ: Type,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Option<Span> {
        match self {
            Expr::BinaryOp { span, .. } => Some(*span),
            Expr::Call { span, .. } => Some(*span),
            Expr::ExternCall { span, .. } => Some(*span),
            Expr::UnaryOp { span, .. } => Some(*span),
            Expr::ArrayInit { span, .. } => Some(*span),
            Expr::Index { span, .. } => Some(*span),
            Expr::If { span, .. } => Some(*span),
            _ => None,
        }
    }

    /// Typ przypisany temu wyrażeniu przez `checker.rs` (otypowany AST).
    /// Przed type-checkingiem to `Type::Unknown`.
    pub fn typ(&self) -> Type {
        match self {
            Expr::Literal { typ, .. }
            | Expr::Variable { typ, .. }
            | Expr::BinaryOp { typ, .. }
            | Expr::Call { typ, .. }
            | Expr::ExternCall { typ, .. }
            | Expr::ArrayInit { typ, .. }
            | Expr::Index { typ, .. }
            | Expr::If { typ, .. }
            | Expr::UnaryOp { typ, .. } => typ.clone(),
        }
    }

    pub fn set_typ(&mut self, t: Type) {
        match self {
            Expr::Literal { typ, .. }
            | Expr::Variable { typ, .. }
            | Expr::BinaryOp { typ, .. }
            | Expr::Call { typ, .. }
            | Expr::ExternCall { typ, .. }
            | Expr::ArrayInit { typ, .. }
            | Expr::Index { typ, .. }
            | Expr::If { typ, .. }
            | Expr::UnaryOp { typ, .. } => *typ = t,
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

/// Operator jednoargumentowy (`-x`, `+x`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Pos,
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
