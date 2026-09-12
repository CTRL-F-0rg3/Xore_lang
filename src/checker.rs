use crate::ast::{Program, Stmt, Expr, Type, BinOp, Literal};
use crate::lexer::Span;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct CheckError {
    pub message: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
struct FunctionInfo {
    params: Vec<Type>,
    return_type: Type,
}

pub struct SemanticChecker {
    scopes: Vec<HashMap<String, VariableInfo>>,
    /// Tablica sygnatur funkcji zebrana z całego programu przed sprawdzeniem
    /// ciał (pozwala na wzajemną rekurencję i wywołania "w przód").
    /// Bez tego `check_expr` dla `Expr::Call` mylił nazwy funkcji ze
    /// zmiennymi lokalnymi - był to błąd w oryginalnej wersji tego pliku.
    functions: HashMap<String, FunctionInfo>,
    pub errors: Vec<CheckError>,
}

#[derive(Debug, Clone, PartialEq)]
struct VariableInfo {
    typ: Type,
    is_mut: bool,
}

impl SemanticChecker {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            functions: HashMap::new(),
            errors: Vec::new(),
        }
    }

    pub fn check_program(&mut self, program: &Program) -> bool {
        // Najpierw zbierz sygnatury wszystkich funkcji (definicji i deklaracji),
        // żeby wywołania (w tym rekurencyjne/wzajemnie rekurencyjne) mogły
        // zostać poprawnie rozpoznane niezależnie od kolejności w pliku.
        for stmt in &program.stmts {
            match stmt {
                Stmt::FnDef { name, params, return_type, .. }
                | Stmt::FnDecl { name, params, return_type, .. } => {
                    self.functions.insert(
                        name.clone(),
                        FunctionInfo {
                            params: params.iter().map(|(_, t)| t.clone()).collect(),
                            return_type: return_type.clone().unwrap_or(Type::Unknown),
                        },
                    );
                }
                _ => {}
            }
        }

        for stmt in &program.stmts {
            self.check_stmt(stmt);
        }
        self.errors.is_empty()
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn get_errors(&self) -> &[CheckError] {
        &self.errors
    }

    fn current_scope_mut(&mut self) -> &mut HashMap<String, VariableInfo> {
        self.scopes.last_mut().unwrap()
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, typ, value, span } => {
                let inferred_type = self.check_expr(value);

                if let Some(expected_type) = typ {
                    if !self.types_match(&inferred_type, expected_type) {
                        self.errors.push(CheckError {
                            message: format!("Niezgodność typów: oczekiwano {:?}, znaleziono {:?}", expected_type, inferred_type),
                                         span: *span,
                        });
                    }
                }

                self.current_scope_mut().insert(
                    name.clone(),
                                                VariableInfo {
                                                    typ: typ.clone().unwrap_or(inferred_type),
                                                is_mut: true,
                                                },
                );
            }
            Stmt::FnDef { params, body, .. } => {
                self.push_scope();

                for (param_name, param_type) in params {
                    self.current_scope_mut().insert(
                        param_name.clone(),
                                                    VariableInfo { typ: param_type.clone(), is_mut: false },
                    );
                }

                for stmt in &body.stmts {
                    self.check_stmt(stmt);
                }

                self.pop_scope();
            }
            Stmt::FnDecl { .. } => {}
            Stmt::Include { .. } => {}
            Stmt::EnumDef { .. } => {}
            Stmt::Expr(expr) => {
                self.check_expr(expr);
            }
        }
    }

    fn check_expr(&mut self, expr: &Expr) -> Type {
        match expr {
            Expr::Literal(lit) => match lit {
                Literal::Int(s) => {
                    if s.contains("_u32") || s.contains("u32") { Type::U32 }
                    else if s.contains("_i64") || s.contains("i64") { Type::I64 }
                    else { Type::I32 }
                }
                Literal::Float(_) => Type::F64,
                Literal::String(_) => Type::Custom("String".to_string()),
                Literal::Bool(_) => Type::Bool,
                Literal::None => Type::Unknown,
            },
            Expr::Variable(name) => {
                for scope in self.scopes.iter().rev() {
                    if let Some(var_info) = scope.get(name) {
                        return var_info.typ.clone();
                    }
                }
                self.errors.push(CheckError {
                    message: format!("Niezdefiniowana zmienna: '{}'", name),
                                 span: expr.span().unwrap_or_default(),
                });
                Type::Unknown
            }
            Expr::BinaryOp { left, op, right, span } => {
                let left_type = self.check_expr(left);
                let right_type = self.check_expr(right);

                if *op == BinOp::SyncAssign {
                    if left_type != right_type {
                        self.errors.push(CheckError {
                            message: format!(
                                "Operator $~ wymaga identycznych typów po obu stronach. Otrzymano: {:?} i {:?}",
                                left_type, right_type
                            ),
                            span: *span,
                        });
                    }

                    if let Expr::Variable(right_name) = right.as_ref() {
                        let is_mut = self.scopes.iter().rev().find_map(|scope| {
                            scope.get(right_name).map(|info| info.is_mut)
                        }).unwrap_or(false);

                        if !is_mut {
                            self.errors.push(CheckError {
                                message: format!("Prawa strona operatora $~ ('{}') musi być modyfikowalna", right_name),
                                             span: *span,
                            });
                        }
                    } else if let Expr::Index { array, .. } = right.as_ref() {
                        // `wartosc $~ tablica[i];` - zapis do elementu
                        // tablicy. Tablica musi być modyfikowalną zmienną
                        // (indeksowanie złożonych wyrażeń nie jest jeszcze
                        // obsługiwane - patrz `lowering.rs`).
                        if let Expr::Variable(arr_name) = array.as_ref() {
                            let is_mut = self.scopes.iter().rev().find_map(|scope| {
                                scope.get(arr_name).map(|info| info.is_mut)
                            });
                            match is_mut {
                                Some(true) => {}
                                Some(false) => {
                                    self.errors.push(CheckError {
                                        message: format!("Tablica '{}' po prawej stronie $~ musi być modyfikowalna", arr_name),
                                        span: *span,
                                    });
                                }
                                None => {
                                    self.errors.push(CheckError {
                                        message: format!("Niezdefiniowana tablica: '{}'", arr_name),
                                        span: *span,
                                    });
                                }
                            }
                        } else {
                            self.errors.push(CheckError {
                                message: "Indeksowanie po prawej stronie $~ jest obsługiwane tylko bezpośrednio na zmiennej tablicowej".to_string(),
                                span: *span,
                            });
                        }
                    } else {
                        self.errors.push(CheckError {
                            message: "Prawa strona operatora $~ musi być zmienną albo elementem tablicy (l-value)".to_string(),
                                         span: *span,
                        });
                    }
                }

                match op {
                    BinOp::SyncAssign => left_type,
                    // Porównania zawsze dają wynik logiczny, niezależnie od
                    // (zgodnych) typów porównywanych operandów.
                    BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge
                    | BinOp::Spaceship => {
                        if left_type != Type::Unknown && right_type != Type::Unknown
                            && !self.types_match(&left_type, &right_type)
                        {
                            self.errors.push(CheckError {
                                message: format!(
                                    "Nie można porównać typów {:?} i {:?}",
                                    left_type, right_type
                                ),
                                span: *span,
                            });
                        }
                        Type::Bool
                    }
                    // Operatory arytmetyczne wymagają typów liczbowych i
                    // zwracają typ operandów (zakładamy, że są zgodne).
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::HashEqEq
                    | BinOp::PipeEq | BinOp::MinusEq => {
                        if left_type != Type::Unknown && !self.is_numeric(&left_type) {
                            self.errors.push(CheckError {
                                message: format!("Operator {:?} wymaga typu liczbowego, znaleziono {:?}", op, left_type),
                                span: *span,
                            });
                        }
                        if left_type != Type::Unknown && right_type != Type::Unknown
                            && !self.types_match(&left_type, &right_type)
                        {
                            self.errors.push(CheckError {
                                message: format!(
                                    "Niezgodność typów operandów {:?}: {:?} i {:?}",
                                    op, left_type, right_type
                                ),
                                span: *span,
                            });
                        }
                        left_type
                    }
                    // Operatory specyficzne dla Xore (@, @/, @=, ~=, :=) - IR
                    // nie posiada dla nich jeszcze zdefiniowanej semantyki
                    // niższego poziomu, więc na razie tylko przepuszczamy typ.
                    _ => left_type,
                }
            }
            Expr::If { condition, then_branch, else_branch, span } => {
                let cond_type = self.check_expr(condition);

                if cond_type != Type::Bool {
                    self.errors.push(CheckError {
                        message: format!("Warunek if musi być typu bool, znaleziono {:?}", cond_type),
                                     span: *span,
                    });
                }

                self.push_scope();
                for stmt in &then_branch.stmts {
                    self.check_stmt(stmt);
                }
                self.pop_scope();

                if let Some(else_block) = else_branch {
                    self.push_scope();
                    for stmt in &else_block.stmts {
                        self.check_stmt(stmt);
                    }
                    self.pop_scope();
                }

                Type::Unknown
            }
            Expr::Call { callee, args, span } => {
                let arg_types: Vec<Type> = args.iter().map(|a| self.check_expr(a)).collect();

                if let Expr::Variable(name) = callee.as_ref() {
                    if let Some(info) = self.functions.get(name).cloned() {
                        if info.params.len() != args.len() {
                            self.errors.push(CheckError {
                                message: format!(
                                    "Funkcja '{}' oczekuje {} argumentów, otrzymano {}",
                                    name, info.params.len(), args.len()
                                ),
                                span: *span,
                            });
                        } else {
                            for (i, (expected, found)) in info.params.iter().zip(arg_types.iter()).enumerate() {
                                if !self.types_match(expected, found) {
                                    self.errors.push(CheckError {
                                        message: format!(
                                            "Argument {} funkcji '{}': oczekiwano {:?}, znaleziono {:?}",
                                            i + 1, name, expected, found
                                        ),
                                        span: *span,
                                    });
                                }
                            }
                        }
                        return info.return_type;
                    } else {
                        self.errors.push(CheckError {
                            message: format!("Wywołanie nieznanej funkcji: '{}'", name),
                            span: *span,
                        });
                        return Type::Unknown;
                    }
                }

                // Wywołanie przez bardziej złożone wyrażenie (nieobsługiwane
                // przez `lowering`, ale sprawdzamy typ dla kompletności).
                self.check_expr(callee);
                Type::Unknown
            }
            Expr::ArrayInit { elements, span } => {
                if let Some(first) = elements.first() {
                    let elem_type = self.check_expr(first);
                    for elem in elements.iter().skip(1) {
                        let t = self.check_expr(elem);
                        if !self.types_match(&elem_type, &t) {
                            // Poprzednio ten błąd był tylko komentarzem i nigdy
                            // nie trafiał do self.errors - tablica mieszanych
                            // typów przechodziła type-checking bez ostrzeżenia.
                            self.errors.push(CheckError {
                                message: format!(
                                    "Niezgodne typy elementów tablicy: oczekiwano {:?}, znaleziono {:?}",
                                    elem_type, t
                                ),
                                span: *span,
                            });
                        }
                    }
                    Type::Array(Box::new(elem_type), Some(elements.len()))
                } else {
                    Type::Array(Box::new(Type::Unknown), Some(0))
                }
            }
            Expr::Index { array, index, span } => {
                let arr_type = self.check_expr(array);
                let idx_type = self.check_expr(index);

                if idx_type != Type::Unknown && !self.is_numeric(&idx_type) {
                    self.errors.push(CheckError {
                        message: format!("Indeks tablicy musi być liczbą całkowitą, znaleziono {:?}", idx_type),
                        span: *span,
                    });
                }

                match arr_type {
                    Type::Array(elem, _) => *elem,
                    Type::Unknown => Type::Unknown,
                    other => {
                        self.errors.push(CheckError {
                            message: format!("Nie można indeksować wartości typu {:?} - to nie tablica", other),
                            span: *span,
                        });
                        Type::Unknown
                    }
                }
            }
        }
    }

    fn types_match(&self, t1: &Type, t2: &Type) -> bool {
        t1 == t2
    }

    fn is_numeric(&self, t: &Type) -> bool {
        matches!(t, Type::I32 | Type::I64 | Type::U32 | Type::U64 | Type::F32 | Type::F64)
    }
}
