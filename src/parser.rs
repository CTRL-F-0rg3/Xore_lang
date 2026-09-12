use crate::lexer::{Lexer, Token, TokenKind, Span};
use crate::ast::*;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

pub struct Parser<'src> {
    tokens: Vec<Token<'src>>,
    current: usize,
    pub errors: Vec<ParseError>,
}

impl<'src> Parser<'src> {
    pub fn new(source: &'src str) -> Self {
        let lexer = Lexer::new(source);
        let tokens: Vec<Token<'src>> = lexer
        .filter_map(|res| match res {
            Ok(tok) if matches!(tok.kind, TokenKind::Whitespace(_) | TokenKind::Comment(_)) => None,
                    Ok(tok) => Some(tok),
                    Err(e) => panic!("Lexer error: {:?}", e),
        })
        .collect();

        Self { tokens, current: 0, errors: Vec::new() }
    }

    pub fn has_errors(&self) -> bool { !self.errors.is_empty() }
    pub fn get_errors(&self) -> &[ParseError] { &self.errors }

    fn peek(&self) -> &TokenKind<'src> {
        if self.current < self.tokens.len() { &self.tokens[self.current].kind } else { &TokenKind::Eof }
    }

    fn peek_span(&self) -> Span {
        if self.current < self.tokens.len() { self.tokens[self.current].span } else { Span { start: 0, end: 0 } }
    }

    fn advance(&mut self) -> &TokenKind<'src> {
        if self.current < self.tokens.len() { self.current += 1; }
        self.peek()
    }

    fn expect(&mut self, expected: TokenKind<'src>) -> Result<(), ParseError> {
        if *self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            let err = ParseError { message: format!("Expected {:?}, found {:?}", expected, self.peek()), span: self.peek_span() };
            self.errors.push(err.clone());
            Err(err)
        }
    }

    pub fn parse_program(&mut self) -> Program {
        let mut stmts = Vec::new();
        while *self.peek() != TokenKind::Eof {
            if let Ok(stmt) = self.parse_stmt() { stmts.push(stmt); }
            else { self.synchronize(); }
        }
        Program { stmts }
    }

    fn synchronize(&mut self) {
        while *self.peek() != TokenKind::Eof {
            if *self.peek() == TokenKind::Semicolon || *self.peek() == TokenKind::RBrace { self.advance(); return; }
            self.advance();
        }
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        let span = self.peek_span();
        match self.peek() {
            TokenKind::KwPublic => {
                self.advance();
                if *self.peek() == TokenKind::KwFn {
                    self.advance();
                    self.parse_fn_def_or_decl(Visibility::Public, span)
                } else {
                    Err(ParseError { message: "Expected 'fn' after 'public'".into(), span })
                }
            }
            TokenKind::KwFn => {
                self.advance();
                self.parse_fn_def_or_decl(Visibility::Private, span)
            }
            TokenKind::KwLet => { self.advance(); self.parse_let(span) }
            TokenKind::KwEnum => { self.advance(); self.parse_enum_def(span) }
            TokenKind::KwInclude => { self.advance(); self.parse_include(span) }
            _ => {
                let expr = self.parse_expr(0)?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Expr(Box::new(expr)))
            }
        }
    }

    fn parse_let(&mut self, span: Span) -> Result<Stmt, ParseError> {
        let name = if let TokenKind::Identifier(n) = self.peek() { n.to_string() } else { return Err(ParseError { message: "Expected identifier after let".into(), span }); };
        self.advance();

        let typ = if *self.peek() == TokenKind::Colon {
            self.advance();
            Some(self.parse_type()?)
        } else { None };

        self.expect(TokenKind::Eq)?;
        let value = Box::new(self.parse_expr(0)?);
        self.expect(TokenKind::Semicolon)?;
        Ok(Stmt::Let { name, typ, value, span })
    }

    fn parse_fn_def_or_decl(&mut self, vis: Visibility, span: Span) -> Result<Stmt, ParseError> {
        let name = if let TokenKind::Identifier(n) = self.peek() { n.to_string() } else { return Err(ParseError { message: "Expected function name".into(), span }); };
        self.advance();

        self.expect(TokenKind::LParen)?;
        let mut params = Vec::new();
        if *self.peek() != TokenKind::RParen {
            loop {
                let param_name = if let TokenKind::Identifier(n) = self.peek() { n.to_string() } else { return Err(ParseError { message: "Expected parameter name".into(), span: self.peek_span() }); };
                self.advance();
                self.expect(TokenKind::Colon)?;
                let param_type = self.parse_type()?;
                params.push((param_name, param_type));
                if *self.peek() == TokenKind::Comma { self.advance(); } else { break; }
            }
        }
        self.expect(TokenKind::RParen)?;

        let return_type = if *self.peek() == TokenKind::ThinArrow {
            self.advance();
            Some(self.parse_type()?)
        } else { None };

        if *self.peek() == TokenKind::Semicolon {
            self.advance();
            Ok(Stmt::FnDecl { visibility: vis, name, params, return_type, span })
        } else {
            let body = self.parse_block()?;
            Ok(Stmt::FnDef { visibility: vis, name, params, return_type, body, span })
        }
    }

    fn parse_include(&mut self, span: Span) -> Result<Stmt, ParseError> {
        let path = if let TokenKind::StringLiteral(s) = self.peek() {
            let clean = s.trim_matches('"');
            clean.to_string()
        } else {
            return Err(ParseError { message: "Expected string literal after 'include'".into(), span });
        };
        self.advance();
        self.expect(TokenKind::Semicolon)?;

        Ok(Stmt::Include { path, span })
    }

    fn parse_enum_def(&mut self, span: Span) -> Result<Stmt, ParseError> {
        let name = if let TokenKind::Identifier(n) = self.peek() { n.to_string() } else { return Err(ParseError { message: "Expected enum name".into(), span }); };
        self.advance();
        self.expect(TokenKind::LBrace)?;
        let mut variants = Vec::new();
        while *self.peek() != TokenKind::RBrace && *self.peek() != TokenKind::Eof {
            if let TokenKind::Identifier(n) = self.peek() { variants.push(n.to_string()); self.advance(); }
            if *self.peek() == TokenKind::Comma { self.advance(); }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(Stmt::EnumDef { name, variants, span })
    }

    fn parse_block(&mut self) -> Result<Block, ParseError> {
        let span = self.peek_span();
        self.expect(TokenKind::LBrace)?;
        let mut stmts = Vec::new();
        while *self.peek() != TokenKind::RBrace && *self.peek() != TokenKind::Eof {
            if let Ok(stmt) = self.parse_stmt() { stmts.push(stmt); } else { self.synchronize(); }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(Block { stmts, span })
    }

    fn parse_type(&mut self) -> Result<Type, ParseError> {
        // Typ tablicowy: `[T; N]` (rozmiar musi być stałą całkowitą znaną w
        // czasie kompilacji - Xore nie ma tablic o dynamicznym rozmiarze).
        if *self.peek() == TokenKind::LBracket {
            let span = self.peek_span();
            self.advance();
            let elem_type = self.parse_type()?;
            self.expect(TokenKind::Semicolon)?;
            let size = if let TokenKind::IntLiteral(n) = self.peek() {
                let clean = n.replace('_', "");
                let v: usize = clean.parse().map_err(|_| ParseError {
                    message: format!("Nieprawidłowy rozmiar tablicy: {:?}", n),
                    span: self.peek_span(),
                })?;
                self.advance();
                v
            } else {
                return Err(ParseError { message: "Expected array size (integer literal)".into(), span: self.peek_span() });
            };
            self.expect(TokenKind::RBracket)?;
            let _ = span;
            return Ok(Type::Array(Box::new(elem_type), Some(size)));
        }

        match self.peek() {
            TokenKind::Identifier(name) => {
                let n = name.to_string();
                self.advance();
                Ok(match n.as_str() {
                    "i32" => Type::I32, "i64" => Type::I64, "u32" => Type::U32, "u64" => Type::U64,
                    "f32" => Type::F32, "f64" => Type::F64, "bool" => Type::Bool, "Self" => Type::Custom("Self".into()),
                   _ => Type::Custom(n),
                })
            }
            _ => Err(ParseError { message: "Expected type".into(), span: self.peek_span() }),
        }
    }

    fn parse_expr(&mut self, min_precedence: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_prefix()?;
        while let Some((op, precedence, associativity)) = self.get_infix_precedence() {
            if precedence < min_precedence { break; }
            let op_span = self.peek_span();
            self.advance();
            let next_min_prec = if associativity == Associativity::Left { precedence + 1 } else { precedence };
            let right = self.parse_expr(next_min_prec)?;
            left = Expr::BinaryOp { left: Box::new(left), op, right: Box::new(right), span: op_span };
        }
        Ok(left)
    }

    fn parse_prefix(&mut self) -> Result<Expr, ParseError> {
        let span = self.peek_span();
        let base = match self.peek() {
            TokenKind::Identifier(name) => {
                let n = name.to_string();
                self.advance();
                if *self.peek() == TokenKind::Dollar {
                    self.advance();
                    self.expect(TokenKind::LParen)?;
                    let args = self.parse_args()?;
                    self.expect(TokenKind::RParen)?;
                    Ok(Expr::Call { callee: Box::new(Expr::Variable(n)), args, span })
                } else {
                    Ok(Expr::Variable(n))
                }
            }
            TokenKind::IntLiteral(n) => { let val = n.to_string(); self.advance(); Ok(Expr::Literal(Literal::Int(val))) }
            TokenKind::FloatLiteral(n) => { let val = n.to_string(); self.advance(); Ok(Expr::Literal(Literal::Float(val))) }
            TokenKind::StringLiteral(n) | TokenKind::RawStringLiteral(n) => { let val = n.to_string(); self.advance(); Ok(Expr::Literal(Literal::String(val))) }
            TokenKind::KwTrue => { self.advance(); Ok(Expr::Literal(Literal::Bool(true))) }
            TokenKind::KwFalse => { self.advance(); Ok(Expr::Literal(Literal::Bool(false))) }
            TokenKind::KwNone => { self.advance(); Ok(Expr::Literal(Literal::None)) }
            TokenKind::KwIf => {
                self.advance();
                self.parse_if_expr(span)
            }
            TokenKind::LParen => { self.advance(); let expr = self.parse_expr(0)?; self.expect(TokenKind::RParen)?; Ok(expr) }
            TokenKind::LBracket => {
                self.advance();
                let mut elements = Vec::new();
                if *self.peek() != TokenKind::RBracket {
                    loop {
                        elements.push(self.parse_expr(0)?);
                        if *self.peek() == TokenKind::Comma { self.advance(); } else { break; }
                    }
                }
                self.expect(TokenKind::RBracket)?;
                Ok(Expr::ArrayInit { elements, span })
            }
            _ => Err(ParseError { message: format!("Unexpected token in expression: {:?}", self.peek()), span }),
        }?;

        // Postfiksowe indeksowanie `wyrazenie[indeks]`, dopuszczalne
        // wielokrotnie (np. na przyszłość dla tablic tablic).
        let mut result = base;
        while *self.peek() == TokenKind::LBracket {
            let idx_span = self.peek_span();
            self.advance();
            let index = self.parse_expr(0)?;
            self.expect(TokenKind::RBracket)?;
            result = Expr::Index { array: Box::new(result), index: Box::new(index), span: idx_span };
        }
        Ok(result)
    }

    fn parse_if_expr(&mut self, span: Span) -> Result<Expr, ParseError> {
        let condition = Box::new(self.parse_expr(0)?);
        let then_branch = self.parse_block()?;

        let else_branch = if *self.peek() == TokenKind::KwElse {
            self.advance();
            Some(self.parse_block()?)
        } else {
            None
        };

        Ok(Expr::If {
            condition,
            then_branch,
            else_branch,
                span,
        })
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if *self.peek() != TokenKind::RParen {
            loop {
                args.push(self.parse_expr(0)?);
                if *self.peek() == TokenKind::Comma { self.advance(); } else { break; }
            }
        }
        Ok(args)
    }

    fn get_infix_precedence(&self) -> Option<(BinOp, u8, Associativity)> {
        match self.peek() {
            TokenKind::DollarTilde => Some((BinOp::SyncAssign, 10, Associativity::Right)),
            TokenKind::ColonEq => Some((BinOp::TildeEq, 15, Associativity::Right)),
            TokenKind::Spaceship => Some((BinOp::Spaceship, 20, Associativity::Left)),
            TokenKind::HashEqEq => Some((BinOp::HashEqEq, 20, Associativity::Left)),
            TokenKind::EqEq => Some((BinOp::Eq, 20, Associativity::Left)),
            TokenKind::NotEq => Some((BinOp::Neq, 20, Associativity::Left)),
            TokenKind::Less => Some((BinOp::Lt, 20, Associativity::Left)),
            TokenKind::Greater => Some((BinOp::Gt, 20, Associativity::Left)),
            TokenKind::LessEq => Some((BinOp::Le, 20, Associativity::Left)),
            TokenKind::GreaterEq => Some((BinOp::Ge, 20, Associativity::Left)),
            TokenKind::PipeEq => Some((BinOp::PipeEq, 30, Associativity::Left)),
            TokenKind::TildeEq => Some((BinOp::TildeEq, 30, Associativity::Left)),
            TokenKind::Plus => Some((BinOp::Add, 40, Associativity::Left)),
            TokenKind::Minus => Some((BinOp::Sub, 40, Associativity::Left)),
            TokenKind::Star => Some((BinOp::Mul, 50, Associativity::Left)),
            TokenKind::Slash => Some((BinOp::Div, 50, Associativity::Left)),
            TokenKind::Percent => Some((BinOp::Mod, 50, Associativity::Left)),
            TokenKind::At => Some((BinOp::At, 60, Associativity::Left)),
            TokenKind::AtSlash => Some((BinOp::AtSlash, 60, Associativity::Left)),
            TokenKind::AtEq => Some((BinOp::AtEq, 60, Associativity::Left)),
            TokenKind::MinusEq => Some((BinOp::MinusEq, 60, Associativity::Left)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Associativity { Left, Right }
