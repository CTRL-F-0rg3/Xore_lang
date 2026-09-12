use crate::ast::{Program, Stmt, Expr, Type, BinOp, Literal, Block};
use crate::ir::*;
use std::collections::HashMap;

pub struct LoweringError {
    pub message: String,
}

pub struct Lowering {
    functions: Vec<IrFunction>,
    current_func: Option<IrFunction>,
    var_map: HashMap<String, Location>,
    /// Tablice: nazwa -> (offset elementu 0, liczba elementów). Osobna mapa,
    /// bo tablica zajmuje wiele kolejnych slotów na stosie, nie jeden - i bo
    /// indeksowanie musi znać jej długość (na przyszłość: sprawdzanie
    /// zakresu) niezależnie od `var_map`.
    array_map: HashMap<String, (i32, usize)>,
    stack_offset: i32,
    next_label_id: u32,
}

/// Parsuje literał całkowity Xore (dziesiętny, `0x`, `0b`, `0o`, z `_` jako
/// separatorem cyfr) na `i64`. Wydzielone jako wolna funkcja, bo potrzebne
/// jest zarówno przy `LoadImm`, jak i przy stałych indeksach tablic
/// (`arr[2]` - patrz `Expr::Index` niżej).
fn parse_int_literal(s: &str) -> i64 {
    let clean: String = s.chars().filter(|&c| c != '_').collect();
    if let Some(rest) = clean.strip_prefix("0x").or_else(|| clean.strip_prefix("0X")) {
        i64::from_str_radix(rest, 16).unwrap_or(0)
    } else if let Some(rest) = clean.strip_prefix("0b").or_else(|| clean.strip_prefix("0B")) {
        i64::from_str_radix(rest, 2).unwrap_or(0)
    } else if let Some(rest) = clean.strip_prefix("0o").or_else(|| clean.strip_prefix("0O")) {
        i64::from_str_radix(rest, 8).unwrap_or(0)
    } else {
        clean.parse().unwrap_or(0)
    }
}

impl Lowering {
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            current_func: None,
            var_map: HashMap::new(),
            array_map: HashMap::new(),
            stack_offset: 0,
            next_label_id: 0,
        }
    }

    pub fn lower_program(&mut self, program: &Program) -> Result<IrProgram, LoweringError> {
        for stmt in &program.stmts {
            self.lower_stmt(stmt)?;
        }

        if let Some(func) = self.current_func.take() {
            self.functions.push(func);
        }

        Ok(IrProgram { functions: self.functions.clone() })
    }

    fn emit(&mut self, instr: IrInstruction) {
        if let Some(func) = &mut self.current_func {
            func.instructions.push(instr);
        }
    }

    fn new_temp(&mut self) -> Temp {
        if let Some(func) = &mut self.current_func {
            func.new_temp()
        } else {
            panic!("No current function")
        }
    }

    fn new_label(&mut self) -> String {
        let label = format!("L{}", self.next_label_id);
        self.next_label_id += 1;
        label
    }

    /// Lowuje blok (ciało funkcji albo gałąź if/else) i zwraca temp
    /// reprezentujący wartość jego ostatniej instrukcji, jeśli jest nią
    /// wyrażenie (niejawna wartość bloku - używane zarówno dla ciał funkcji,
    /// jak i gałęzi `if`/`else`, które w Xore też są wyrażeniami).
    fn lower_block_value(&mut self, stmts: &[Stmt]) -> Result<Option<Temp>, LoweringError> {
        let mut last_expr_value: Option<Temp> = None;
        for (i, stmt) in stmts.iter().enumerate() {
            let is_last = i + 1 == stmts.len();
            if is_last {
                if let Stmt::Expr(expr) = stmt {
                    last_expr_value = Some(self.lower_expr(expr)?);
                    continue;
                }
            }
            last_expr_value = None;
            self.lower_stmt(stmt)?;
        }
        Ok(last_expr_value)
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), LoweringError> {
        match stmt {
            Stmt::FnDef { name, params, return_type, body, .. } => {
                if let Some(func) = self.current_func.take() {
                    self.functions.push(func);
                }

                self.current_func = Some(IrFunction {
                    name: name.clone(),
                                         params: params.clone(),
                                         return_type: return_type.clone(),
                                         locals: Vec::new(),
                                         instructions: Vec::new(),
                                         next_temp: 0,
                });
                self.var_map.clear();
                self.stack_offset = 0;

                self.emit(IrInstruction::Label(format!("func_{}", name)));

                // Powiąż parametry wejściowe (rejestry wg konwencji wywołań) z ich
                // slotami na stosie. To był brakujący krok - bez niego LoadMem na
                // parametrze czytałby niezainicjalizowaną pamięć.
                for (i, (param_name, param_type)) in params.iter().enumerate() {
                    self.stack_offset -= 8;
                    let loc = Location::StackSlot(self.stack_offset);
                    self.var_map.insert(param_name.clone(), loc.clone());

                    if let Some(func) = &mut self.current_func {
                        func.locals.push((param_name.clone(), param_type.clone()));
                    }

                    let ptemp = self.new_temp();
                    self.emit(IrInstruction::LoadParam { dst: ptemp, index: i });
                    self.emit(IrInstruction::StoreMem { dst: loc, src: ptemp });
                }

                // Ostatnia instrukcja-wyrażenie w ciele funkcji jest niejawną
                // wartością zwracaną (tak jak w przykładach: `a + b;` na końcu `add`).
                let return_value = self.lower_block_value(&body.stmts)?;

                let already_returns = self.current_func.as_ref()
                .map(|f| matches!(f.instructions.last(), Some(IrInstruction::Return(_))))
                .unwrap_or(false);

                if !already_returns {
                    if return_type.is_some() {
                        self.emit(IrInstruction::Return(return_value));
                    } else {
                        self.emit(IrInstruction::Return(None));
                    }
                }
            }
            Stmt::Let { name, value, .. } => {
                // Tablica ma osobną ścieżkę: potrzebuje N kolejnych slotów
                // (nie jednego) i wpisu w `array_map`, żeby `Expr::Index`
                // później wiedział, gdzie jej szukać. Obsługiwane jest dziś
                // wyłącznie `let arr: [T; N] = [e0, e1, ...];` - przypisanie
                // całej tablicy z innej zmiennej albo zwrócenie tablicy z
                // funkcji nie jest jeszcze wspierane (patrz README).
                if let Expr::ArrayInit { elements, .. } = value.as_ref() {
                    let mut elem0_offset = 0;
                    for (i, elem) in elements.iter().enumerate() {
                        self.stack_offset -= 8;
                        if i == 0 {
                            elem0_offset = self.stack_offset;
                        }
                        let slot = Location::StackSlot(self.stack_offset);
                        if let Some(func) = &mut self.current_func {
                            func.locals.push((format!("{}[{}]", name, i), Type::Unknown));
                        }
                        let val_temp = self.lower_expr(elem)?;
                        self.emit(IrInstruction::StoreMem { dst: slot, src: val_temp });
                    }
                    self.array_map.insert(name.clone(), (elem0_offset, elements.len()));
                    return Ok(());
                }

                self.stack_offset -= 8;
                let loc = Location::StackSlot(self.stack_offset);
                self.var_map.insert(name.clone(), loc.clone());

                if let Some(func) = &mut self.current_func {
                    func.locals.push((name.clone(), Type::Unknown));
                }

                let val_temp = self.lower_expr(value)?;

                self.emit(IrInstruction::StoreMem { dst: loc, src: val_temp });
            }
            Stmt::FnDecl { .. } => {}
            Stmt::Include { .. } => {}
            Stmt::Expr(expr) => {
                self.lower_expr(expr)?;
            }
            Stmt::EnumDef { .. } => {}
        }
        Ok(())
    }

    fn lower_expr(&mut self, expr: &Expr) -> Result<Temp, LoweringError> {
        match expr {
            Expr::Literal(lit) => {
                let dst = self.new_temp();
                let value = match lit {
                    Literal::Int(s) => Operand::ImmInt(parse_int_literal(s)),
                    Literal::Float(s) => Operand::ImmFloat(s.parse().unwrap_or(0.0)),
                    Literal::String(s) => Operand::ImmString(s.clone()),
                    _ => Operand::ImmInt(0),
                };
                self.emit(IrInstruction::LoadImm { dst, value });
                Ok(dst)
            }
            Expr::Variable(name) => {
                if let Some(loc) = self.var_map.get(name).cloned() {
                    let dst = self.new_temp();
                    self.emit(IrInstruction::LoadMem { dst, src: loc });
                    Ok(dst)
                } else {
                    Err(LoweringError { message: format!("Undefined variable: {}", name) })
                }
            }
            Expr::BinaryOp { left, op, right, .. } => {
                let left_temp = self.lower_expr(left)?;
                let right_temp = self.lower_expr(right)?;

                if *op == BinOp::SyncAssign {
                    match right.as_ref() {
                        Expr::Variable(right_name) => {
                            if let Some(loc) = self.var_map.get(right_name).cloned() {
                                self.emit(IrInstruction::StoreMem { dst: loc, src: left_temp });
                                Ok(left_temp)
                            } else {
                                Err(LoweringError { message: format!("Undefined variable: {}", right_name) })
                            }
                        }
                        Expr::Index { array, index, .. } => {
                            self.lower_index_store(array, index, left_temp)?;
                            Ok(left_temp)
                        }
                        _ => Err(LoweringError { message: "Right side of $~ must be a variable or an array element".to_string() }),
                    }
                } else {
                    let dst = self.new_temp();
                    let ir_op = match op {
                        BinOp::Add => IrBinOp::Add,
                        BinOp::Sub => IrBinOp::Sub,
                        BinOp::Mul => IrBinOp::Mul,
                        BinOp::Div => IrBinOp::Div,
                        BinOp::Mod => IrBinOp::Mod,
                        BinOp::Eq => IrBinOp::Eq,
                        BinOp::Neq => IrBinOp::Neq,
                        BinOp::Lt => IrBinOp::Lt,
                        BinOp::Gt => IrBinOp::Gt,
                        BinOp::Le => IrBinOp::Le,
                        BinOp::Ge => IrBinOp::Ge,
                        _ => return Err(LoweringError { message: format!("Unsupported operator: {:?}", op) }),
                    };
                    self.emit(IrInstruction::BinaryOp { dst, left: left_temp, op: ir_op, right: right_temp });
                    Ok(dst)
                }
            }
            Expr::If { condition, then_branch, else_branch, span: _ } => {
                let cond_temp = self.lower_expr(condition)?;

                let then_label = self.new_label();
                let else_label = self.new_label();
                let end_label = self.new_label();

                // Slot na wynik if-wyrażenia: obie gałęzie zapisują tu swoją
                // wartość przed skokiem na koniec - to najprostszy poprawny
                // odpowiednik phi-node bez wprowadzania pełnej SSA w IR.
                self.stack_offset -= 8;
                let result_slot = Location::StackSlot(self.stack_offset);
                if let Some(func) = &mut self.current_func {
                    func.locals.push(("<if_result>".to_string(), Type::Unknown));
                }

                // Zeruj slot PRZED gałęzią: jeśli `if` jest użyte jako
                // wartość, ale któraś gałąź (albo brakujący `else`) nie
                // kończy się wyrażeniem, LoadMem po `end_label` czytałby
                // niezainicjalizowaną pamięć stosu. To był realny, cichy
                // błąd (wykryty przy pisaniu dokumentacji) - teraz zamiast
                // śmieci dostaje się bezpieczne `0`.
                let zero_temp = self.new_temp();
                self.emit(IrInstruction::LoadImm { dst: zero_temp, value: Operand::ImmInt(0) });
                self.emit(IrInstruction::StoreMem { dst: result_slot.clone(), src: zero_temp });

                self.emit(IrInstruction::JumpIfZero { cond: cond_temp, target: else_label.clone() });

                self.emit(IrInstruction::Label(then_label));
                let then_val = self.lower_block_value(&then_branch.stmts)?;
                if let Some(v) = then_val {
                    self.emit(IrInstruction::StoreMem { dst: result_slot.clone(), src: v });
                }
                self.emit(IrInstruction::Jump(end_label.clone()));

                self.emit(IrInstruction::Label(else_label));
                let else_val = if let Some(else_block) = else_branch {
                    self.lower_block_value(&else_block.stmts)?
                } else {
                    None
                };
                if let Some(v) = else_val {
                    self.emit(IrInstruction::StoreMem { dst: result_slot.clone(), src: v });
                }

                self.emit(IrInstruction::Label(end_label));

                let dst = self.new_temp();
                self.emit(IrInstruction::LoadMem { dst, src: result_slot });
                Ok(dst)
            }
            Expr::Call { callee, args, .. } => {
                let func_name = if let Expr::Variable(name) = callee.as_ref() {
                    name.clone()
                } else {
                    return Err(LoweringError { message: "Only direct function calls supported".to_string() });
                };

                let mut arg_temps = Vec::new();
                for arg in args {
                    arg_temps.push(self.lower_expr(arg)?);
                }

                let dst = self.new_temp();
                self.emit(IrInstruction::Call { dst: Some(dst), func: func_name, args: arg_temps });
                Ok(dst)
            }
            Expr::ArrayInit { .. } => {
                // Literał tablicowy jest w pełni obsługiwany tylko jako
                // bezpośrednia wartość `let arr: [T;N] = [...]` (patrz
                // Stmt::Let wyżej), bo tam wiemy, gdzie fizycznie ulokować
                // jego elementy. Użyty w jakimkolwiek innym miejscu (jako
                // argument wywołania, zagnieżdżony w innym wyrażeniu, itp.)
                // nie ma dokąd "wylądować" - to świadomie nieobsługiwany
                // przypadek (patrz README), zwracamy stałe 0 zamiast fałszywie
                // udawać, że coś sensownego się stało.
                let dst = self.new_temp();
                self.emit(IrInstruction::LoadImm { dst, value: Operand::ImmInt(0) });
                Ok(dst)
            }
            Expr::Index { array, index, .. } => {
                let name = if let Expr::Variable(n) = array.as_ref() {
                    n.clone()
                } else {
                    return Err(LoweringError {
                        message: "Indeksowanie jest dziś obsługiwane tylko bezpośrednio na zmiennej tablicowej (np. `arr[i]`, nie `f()[i]`)".to_string(),
                    });
                };
                let (elem0_offset, _len) = *self.array_map.get(&name).ok_or_else(|| LoweringError {
                    message: format!("'{}' nie jest zainicjalizowaną tablicą lokalną", name),
                })?;

                // Indeks znany w czasie kompilacji: zamień na zwykły
                // LoadMem pod stałym adresem - korzysta z istniejących
                // optymalizacji (store->load forwarding, DCE) tak samo jak
                // każda inna zmienna, zamiast zawsze liczyć adres w runtime.
                if let Expr::Literal(Literal::Int(s)) = index.as_ref() {
                    let idx = parse_int_literal(s);
                    let dst = self.new_temp();
                    self.emit(IrInstruction::LoadMem {
                        dst,
                        src: Location::StackSlot(elem0_offset - 8 * (idx as i32)),
                    });
                    return Ok(dst);
                }

                let idx_temp = self.lower_expr(index)?;
                let dst = self.new_temp();
                self.emit(IrInstruction::LoadIndexed { dst, base_offset: elem0_offset, index: idx_temp, elem_size: 8 });
                Ok(dst)
            }
        }
    }

    /// Loweruje zapis `wartosc $~ tablica[indeks];` - odpowiednik `StoreMem`
    /// / `StoreIndexed` dla elementu tablicy, używany przez `BinaryOp` z
    /// `SyncAssign` w przypadku, gdy prawa strona jest indeksowaniem.
    fn lower_index_store(&mut self, array: &Expr, index: &Expr, src: Temp) -> Result<(), LoweringError> {
        let name = if let Expr::Variable(n) = array {
            n.clone()
        } else {
            return Err(LoweringError {
                message: "Zapis przez indeks jest dziś obsługiwany tylko bezpośrednio na zmiennej tablicowej".to_string(),
            });
        };
        let (elem0_offset, _len) = *self.array_map.get(&name).ok_or_else(|| LoweringError {
            message: format!("'{}' nie jest zainicjalizowaną tablicą lokalną", name),
        })?;

        if let Expr::Literal(Literal::Int(s)) = index {
            let idx = parse_int_literal(s);
            self.emit(IrInstruction::StoreMem {
                dst: Location::StackSlot(elem0_offset - 8 * (idx as i32)),
                src,
            });
            return Ok(());
        }

        let idx_temp = self.lower_expr(index)?;
        self.emit(IrInstruction::StoreIndexed { base_offset: elem0_offset, index: idx_temp, src, elem_size: 8 });
        Ok(())
    }
}
