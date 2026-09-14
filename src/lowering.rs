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
    /// Tablice lokalne (zadeklarowane przez `let arr: [T;N] = [...]` w tej
    /// funkcji): nazwa -> (offset elementu 0, liczba elementów). Osobna
    /// mapa, bo tablica zajmuje wiele kolejnych slotów na stosie, nie jeden.
    array_map: HashMap<String, (i32, usize)>,
    /// Parametry funkcji typu tablicowego (`fn f(a: [i32;3])`): nazwa ->
    /// liczba elementów. Taki parametr "rozpada się" do wskaźnika (adresu
    /// elementu 0 w ramce WYWOŁUJĄCEGO) - jego wartość jest zwykłym
    /// skalarem w `var_map` (jak każdy inny parametr), ale `Expr::Index`
    /// musi wiedzieć, że trzeba adresować PRZEZ tę wartość (`LoadIndexedPtr`),
    /// a nie traktować ją jak zwykłą liczbę. Resetowana na starcie każdej funkcji.
    ptr_array_params: std::collections::HashSet<String>,
    /// Typ każdej zmiennej lokalnej/parametru w BIEŻĄCEJ funkcji (jawna
    /// adnotacja z `let`, albo wywnioskowany). Potrzebne, żeby wiedzieć,
    /// czy operacja binarna jest na `u32`/`u64` (wymaga wariantu IR bez
    /// znaku - patrz `IrBinOp::*U`) czy na typie ze znakiem. Resetowana na
    /// starcie każdej funkcji.
    ///
    /// UWAGA architektoniczna: to jest niezależne, uproszczone
    /// przybliżenie type-checkingu, wykonane osobno wewnątrz lowering (bo
    /// `checker.rs` nie zostawia żadnej adnotacji na AST, z której lowering
    /// mogłoby skorzystać). To duplikacja logiki - docelowo `checker.rs`
    /// powinien produkować otypowane AST/IR, które lowering by tylko
    /// konsumowało, zamiast zgadywać typy drugi raz. Zostawione jako
    /// świadomy dług techniczny (patrz README), bo pełne rozwiązanie
    /// (przeciągnięcie typów przez całe IR - w tym float) to dużo większa
    /// zmiana niż zakres tej rundy.
    var_types: HashMap<String, Type>,
    /// Typ zwracany każdej funkcji w programie (z `FnDef`/`FnDecl`),
    /// zebrany raz na starcie `lower_program` - potrzebne do wnioskowania
    /// typu wyniku wywołania (`Expr::Call`).
    fn_return_types: HashMap<String, Type>,
    stack_offset: i32,
    next_label_id: u32,
}

/// Parsuje literał całkowity (patrz `crate::numlit`) i zwraca samą wartość -
/// wygodny skrót dla miejsc w tym pliku, które potrzebują tylko `i64`
/// (stałe indeksy tablic), nie pełnego typu.
fn parse_int_literal(s: &str) -> i64 {
    crate::numlit::parse_int_literal(s).bits
}

impl Lowering {
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            current_func: None,
            var_map: HashMap::new(),
            array_map: HashMap::new(),
            ptr_array_params: std::collections::HashSet::new(),
            var_types: HashMap::new(),
            fn_return_types: HashMap::new(),
            stack_offset: 0,
            next_label_id: 0,
        }
    }

    /// Wnioskuje (przybliżony) typ wyrażenia - patrz komentarz przy
    /// `var_types` odnośnie tego, dlaczego to osobna, uproszczona kopia
    /// tego, co już policzył `checker.rs`.
    fn infer_expr_type(&self, expr: &Expr) -> Type {
        match expr {
            Expr::Literal(Literal::Int(s)) => {
                crate::numlit::infer_int_type(&crate::numlit::parse_int_literal(s))
            }
            Expr::Literal(Literal::Bool(_)) => Type::Bool,
            Expr::Literal(Literal::Float(_)) => Type::F64,
            Expr::Literal(Literal::String(_)) => Type::Custom("String".to_string()),
            Expr::Literal(Literal::None) => Type::Unknown,
            Expr::Variable(name) => self.var_types.get(name).cloned().unwrap_or(Type::Unknown),
            Expr::BinaryOp { left, op, .. } => match op {
                BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => Type::Bool,
                _ => self.infer_expr_type(left),
            },
            Expr::Call { callee, .. } => {
                if let Expr::Variable(name) = callee.as_ref() {
                    self.fn_return_types.get(name).cloned().unwrap_or(Type::Unknown)
                } else {
                    Type::Unknown
                }
            }
            _ => Type::Unknown,
        }
    }

    pub fn lower_program(&mut self, program: &Program) -> Result<IrProgram, LoweringError> {
        // Zbierz typy zwracane wszystkich funkcji naprzód, żeby wywołania
        // "w przód" (i wzajemnie rekurencyjne) też miały poprawnie
        // wywnioskowany typ wyniku.
        for stmt in &program.stmts {
            match stmt {
                Stmt::FnDef { name, return_type, .. } | Stmt::FnDecl { name, return_type, .. } => {
                    self.fn_return_types.insert(name.clone(), return_type.clone().unwrap_or(Type::Unknown));
                }
                _ => {}
            }
        }

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

                // Zwracanie tablic z funkcji wymagałoby konwencji ABI typu
                // "sret" (wywołujący rezerwuje miejsce i przekazuje adres),
                // której dziś nie mamy. Zamiast po cichu wygenerować zły
                // kod (np. zwrócić tylko pierwszy element), odmawiamy
                // kompilacji tego przypadku wprost.
                if let Some(Type::Array(..)) = return_type {
                    return Err(LoweringError {
                        message: format!(
                            "Funkcja '{}': zwracanie tablic z funkcji nie jest jeszcze obsługiwane",
                            name
                        ),
                    });
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
                // `array_map`/`ptr_array_params` też muszą być czyszczone na
                // starcie każdej funkcji - inaczej tablica lokalna z
                // poprzednio lowerowanej funkcji zostawiałaby "duchowy" wpis
                // ze starym (nieaktualnym w nowej ramce) offsetem, co przy
                // koincydencji nazw dałoby błędny adres. To był realny,
                // utajony bug (nigdy się nie ujawnił, bo dotychczasowe testy
                // nie używały tej samej nazwy tablicy w dwóch funkcjach).
                self.array_map.clear();
                self.ptr_array_params.clear();
                self.var_types.clear();
                self.stack_offset = 0;

                self.emit(IrInstruction::Label(format!("func_{}", name)));

                // Powiąż parametry wejściowe (rejestry wg konwencji wywołań) z ich
                // slotami na stosie. To był brakujący krok - bez niego LoadMem na
                // parametrze czytałby niezainicjalizowaną pamięć.
                for (i, (param_name, param_type)) in params.iter().enumerate() {
                    self.stack_offset -= 8;
                    let loc = Location::StackSlot(self.stack_offset);
                    self.var_map.insert(param_name.clone(), loc.clone());
                    self.var_types.insert(param_name.clone(), param_type.clone());

                    if let Some(func) = &mut self.current_func {
                        func.locals.push((param_name.clone(), param_type.clone()));
                    }

                    let ptemp = self.new_temp();
                    self.emit(IrInstruction::LoadParam { dst: ptemp, index: i });
                    self.emit(IrInstruction::StoreMem { dst: loc, src: ptemp });

                    // Parametr tablicowy "rozpada się" do wskaźnika (adresu
                    // elementu 0 w ramce wywołującego) - jego WARTOŚĆ jest
                    // zwykłym skalarem (obsłużonym wyżej jak każdy inny
                    // parametr), ale trzeba zapamiętać, że indeksowanie tej
                    // nazwy musi iść przez `LoadIndexedPtr`, nie przez stały
                    // offset w bieżącej ramce.
                    if matches!(param_type, Type::Array(..)) {
                        self.ptr_array_params.insert(param_name.clone());
                    }
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
            Stmt::Let { name, typ, value, .. } => {
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

                // Kopia całej tablicy lokalnej: `let b = a;` (albo z jawną
                // adnotacją typu) gdy `a` jest znaną tablicą lokalną -
                // kopiujemy jej N elementów do N nowych slotów. Uwaga: jeśli
                // `a` jest parametrem tablicowym (wskaźnikiem - patrz
                // `ptr_array_params`), to NIE trafia tutaj, tylko w zwykłą
                // ścieżkę skalarną niżej, więc `b` dostanie kopię WSKAŹNIKA
                // (alias tej samej pamięci), nie głęboką kopię - spójne z
                // tym, że parametr tablicowy i tak jest już tylko wskaźnikiem
                // (patrz README, sekcja o tablicach).
                if let Expr::Variable(src_name) = value.as_ref() {
                    if let Some(&(src_elem0, len)) = self.array_map.get(src_name) {
                        let mut dst_elem0_offset = 0;
                        for i in 0..len {
                            let val_temp = self.new_temp();
                            self.emit(IrInstruction::LoadMem {
                                dst: val_temp,
                                src: Location::StackSlot(src_elem0 - 8 * (i as i32)),
                            });
                            self.stack_offset -= 8;
                            if i == 0 {
                                dst_elem0_offset = self.stack_offset;
                            }
                            let dst_slot = Location::StackSlot(self.stack_offset);
                            if let Some(func) = &mut self.current_func {
                                func.locals.push((format!("{}[{}]", name, i), Type::Unknown));
                            }
                            self.emit(IrInstruction::StoreMem { dst: dst_slot, src: val_temp });
                        }
                        self.array_map.insert(name.clone(), (dst_elem0_offset, len));
                        return Ok(());
                    }
                }

                self.stack_offset -= 8;
                let loc = Location::StackSlot(self.stack_offset);
                self.var_map.insert(name.clone(), loc.clone());
                // Zapamiętaj typ zmiennej (jawna adnotacja albo
                // wywnioskowany) - potrzebne przy operacjach binarnych, żeby
                // wiedzieć, czy użyć wariantu IR ze znakiem czy bez (patrz
                // `var_types` i `IrBinOp::*U`).
                let var_ty = typ.clone().unwrap_or_else(|| self.infer_expr_type(value));
                self.var_types.insert(name.clone(), var_ty);

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
                } else if let Some(&(elem0_offset, _len)) = self.array_map.get(name) {
                    // Tablica lokalna użyta jako zwykła wartość (np. argument
                    // wywołania funkcji) "rozpada się" do wskaźnika - adresu
                    // jej elementu 0 - dokładnie tak jak tablica w C. To
                    // jedyny sposób przekazania tablicy lokalnej do innej
                    // funkcji (patrz `ptr_array_params` i `Stmt::FnDef`).
                    let dst = self.new_temp();
                    self.emit(IrInstruction::LoadAddr { dst, base_offset: elem0_offset });
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
                    // Porównania/dzielenie muszą użyć wariantu IR bez znaku,
                    // gdy operandy są `u32`/`u64` - inaczej np. `4000000000_u32
                    // < 10` (gdzie lewa wartość ma ustawiony najstarszy bit)
                    // dałoby błędny wynik przy zwykłym porównaniu ze znakiem.
                    // Sprawdzamy tylko `left` - checker już wymusił, że oba
                    // operandy mają ten sam typ.
                    let unsigned = crate::numlit::is_unsigned(&self.infer_expr_type(left));
                    let ir_op = match op {
                        BinOp::Add => IrBinOp::Add,
                        BinOp::Sub => IrBinOp::Sub,
                        BinOp::Mul => IrBinOp::Mul,
                        BinOp::Div => if unsigned { IrBinOp::DivU } else { IrBinOp::Div },
                        BinOp::Mod => if unsigned { IrBinOp::ModU } else { IrBinOp::Mod },
                        BinOp::Eq => IrBinOp::Eq,
                        BinOp::Neq => IrBinOp::Neq,
                        BinOp::Lt => if unsigned { IrBinOp::LtU } else { IrBinOp::Lt },
                        BinOp::Gt => if unsigned { IrBinOp::GtU } else { IrBinOp::Gt },
                        BinOp::Le => if unsigned { IrBinOp::LeU } else { IrBinOp::Le },
                        BinOp::Ge => if unsigned { IrBinOp::GeU } else { IrBinOp::Ge },
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

                // Przypadek 1: tablica lokalna (`let arr = [...]`) - stały,
                // znany w czasie kompilacji offset w bieżącej ramce.
                if let Some(&(elem0_offset, _len)) = self.array_map.get(&name) {
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
                    return Ok(dst);
                }

                // Przypadek 2: parametr tablicowy - wartość to WSKAŹNIK
                // (adres w ramce WYWOŁUJĄCEGO), znany dopiero w runtime.
                if self.ptr_array_params.contains(&name) {
                    let base_loc = self.var_map.get(&name).cloned().ok_or_else(|| LoweringError {
                        message: format!("Wewnętrzny błąd: brak lokalizacji parametru '{}'", name),
                    })?;
                    let base_temp = self.new_temp();
                    self.emit(IrInstruction::LoadMem { dst: base_temp, src: base_loc });

                    let idx_temp = self.lower_expr(index)?;
                    let dst = self.new_temp();
                    self.emit(IrInstruction::LoadIndexedPtr { dst, base: base_temp, index: idx_temp, elem_size: 8 });
                    return Ok(dst);
                }

                Err(LoweringError {
                    message: format!("'{}' nie jest ani zainicjalizowaną tablicą lokalną, ani parametrem tablicowym", name),
                })
            }
        }
    }

    /// Loweruje zapis `wartosc $~ tablica[indeks];` - odpowiednik `StoreMem`
    /// / `StoreIndexed` / `StoreIndexedPtr` dla elementu tablicy, używany
    /// przez `BinaryOp` z `SyncAssign` w przypadku, gdy prawa strona jest
    /// indeksowaniem.
    fn lower_index_store(&mut self, array: &Expr, index: &Expr, src: Temp) -> Result<(), LoweringError> {
        let name = if let Expr::Variable(n) = array {
            n.clone()
        } else {
            return Err(LoweringError {
                message: "Zapis przez indeks jest dziś obsługiwany tylko bezpośrednio na zmiennej tablicowej".to_string(),
            });
        };

        if let Some(&(elem0_offset, _len)) = self.array_map.get(&name) {
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
            return Ok(());
        }

        if self.ptr_array_params.contains(&name) {
            let base_loc = self.var_map.get(&name).cloned().ok_or_else(|| LoweringError {
                message: format!("Wewnętrzny błąd: brak lokalizacji parametru '{}'", name),
            })?;
            let base_temp = self.new_temp();
            self.emit(IrInstruction::LoadMem { dst: base_temp, src: base_loc });
            let idx_temp = self.lower_expr(index)?;
            self.emit(IrInstruction::StoreIndexedPtr { base: base_temp, index: idx_temp, src, elem_size: 8 });
            return Ok(());
        }

        Err(LoweringError {
            message: format!("'{}' nie jest ani zainicjalizowaną tablicą lokalną, ani parametrem tablicowym", name),
        })
    }
}
