//! Generator kodu x86_64 (System V AMD64 ABI, składnia AT&T, kompatybilna z GNU `as`).
//!
//! Strategia rejestrów: tempy IR trzymamy WYŁĄCZNIE w rejestrach
//! zachowywanych przez wywoływanego: %rbx, %r12, %r13, %r14, %r15.
//! Rejestry przenoszące argumenty (%rdi,%rsi,%rdx,%rcx,%r8,%r9) i zwracaną
//! wartość (%rax) są rozłączne z tą pulą, więc przekazywanie argumentów
//! przy wywołaniu funkcji nigdy nie wymaga rozwiązywania konfliktów kolejności.
//!
//! Ograniczenia (świadome, udokumentowane - patrz README wygenerowane niżej
//! w komentarzu na końcu pliku): brak natywnej arytmetyki
//! zmiennoprzecinkowej (IR nie niesie informacji o typie operacji - to
//! architektoniczna luka w dostarczonym `ir.rs`/`lowering.rs`, nie w tym
//! generatorze), maksymalnie 6 argumentów całkowitych na wywołanie.

use crate::ir::*;
use crate::regalloc::{self, Home};
use std::collections::HashMap;
use std::fmt::Write as _;

const POOL: [&str; 5] = ["%rbx", "%r12", "%r13", "%r14", "%r15"];
const POOL_BYTE: [&str; 5] = ["%bl", "%r12b", "%r13b", "%r14b", "%r15b"];
const ARG_REGS: [&str; 6] = ["%rdi", "%rsi", "%rdx", "%rcx", "%r8", "%r9"];
const SCRATCH1: &str = "%rax";
const SCRATCH1_B: &str = "%al";
const SCRATCH2: &str = "%r10";
const SCRATCH2_B: &str = "%r10b";
const POOL_SAVE_BYTES: i32 = (POOL.len() as i32) * 8; // + rbp już wypchnięty osobno

pub struct X86_64CodeGen {
    out: String,
    strings: Vec<String>,
    string_labels: HashMap<String, String>,
}

impl X86_64CodeGen {
    pub fn new() -> Self {
        Self { out: String::new(), strings: Vec::new(), string_labels: HashMap::new() }
    }

    pub fn compile(mut self, program: &IrProgram) -> String {
        writeln!(self.out, "    .text").unwrap();

        // Punkt wejścia programu: Xore jest zawsze budowany jako plik
        // freestanding (bez libc, bez crt0) - to my dostarczamy `_start`,
        // wołamy funkcję `main` napisaną w Xore, a jej wynik (%rax)
        // przekazujemy bezpośrednio do syscalla exit. Zero zależności od
        // dynamicznego linkera czy jakiejkolwiek biblioteki C.
        writeln!(self.out, "    .globl _start").unwrap();
        writeln!(self.out, "_start:").unwrap();
        writeln!(self.out, "    call main").unwrap();
        writeln!(self.out, "    movq %rax, %rdi").unwrap();
        writeln!(self.out, "    movq $60, %rax    # sys_exit").unwrap();
        writeln!(self.out, "    syscall").unwrap();

        for func in &program.functions {
            self.compile_function(func);
        }

        if !self.strings.is_empty() {
            writeln!(self.out, "\n    .section .rodata").unwrap();
            for (i, s) in self.strings.iter().enumerate() {
                writeln!(self.out, ".Lstr{}:\n    .asciz {:?}", i, s).unwrap();
            }
        }
        self.out
    }

    fn intern_string(&mut self, s: &str) -> String {
        if let Some(l) = self.string_labels.get(s) {
            return l.clone();
        }
        let label = format!(".Lstr{}", self.strings.len());
        self.strings.push(s.to_string());
        self.string_labels.insert(s.to_string(), label.clone());
        label
    }

    fn round16(n: i32) -> i32 {
        (n + 15) & !15
    }

    fn slot_addr(off: i32) -> String {
        // off to np. -8, -16, ... (patrz lowering.rs)
        format!("-{}(%rbp)", POOL_SAVE_BYTES + (-off))
    }

    fn spill_addr(locals_bytes: i32, k: usize) -> String {
        format!("-{}(%rbp)", POOL_SAVE_BYTES + locals_bytes + 8 * (k as i32 + 1))
    }

    fn label(func_name: &str, l: &str) -> String {
        format!(".L{}_{}", func_name, l)
    }

    /// Zwraca napis, którego można użyć jako operand źródłowy dla `t`
    /// (bez emitowania kodu, jeśli `t` mieszka w rejestrze; w przeciwnym
    /// razie ładuje wartość ze slotu rozlania do `scratch` i zwraca `scratch`).
    fn read(&mut self, alloc: &regalloc::RegAllocResult, locals_bytes: i32, t: Temp, scratch: &str) -> String {
        match alloc.home[&t] {
            Home::Reg(r) => POOL[r].to_string(),
            Home::Spill(k) => {
                writeln!(self.out, "    movq {}, {}", Self::spill_addr(locals_bytes, k), scratch).unwrap();
                scratch.to_string()
            }
        }
    }

    /// Zapisuje wartość z rejestru `src_reg` do miejsca zamieszkania `t`.
    fn write_home(&mut self, alloc: &regalloc::RegAllocResult, locals_bytes: i32, t: Temp, src_reg: &str) {
        match alloc.home[&t] {
            Home::Reg(r) => {
                if POOL[r] != src_reg {
                    writeln!(self.out, "    movq {}, {}", src_reg, POOL[r]).unwrap();
                }
            }
            Home::Spill(k) => {
                writeln!(self.out, "    movq {}, {}", src_reg, Self::spill_addr(locals_bytes, k)).unwrap();
            }
        }
    }

    /// Rejestr roboczy do zapisania wyniku instrukcji: jeśli dst mieszka
    /// w rejestrze puli, pracujemy bezpośrednio na nim; inaczej używamy SCRATCH1.
    fn work_reg(alloc: &regalloc::RegAllocResult, t: Temp) -> &'static str {
        match alloc.home[&t] {
            Home::Reg(r) => POOL[r],
            Home::Spill(_) => SCRATCH1,
        }
    }

    fn byte_of(reg: &str) -> &'static str {
        match reg {
            "%rax" => SCRATCH1_B,
            "%r10" => SCRATCH2_B,
            "%rbx" => POOL_BYTE[0],
            "%r12" => POOL_BYTE[1],
            "%r13" => POOL_BYTE[2],
            "%r14" => POOL_BYTE[3],
            "%r15" => POOL_BYTE[4],
            _ => unreachable!("nieoczekiwany rejestr roboczy: {}", reg),
        }
    }

    fn compile_function(&mut self, func: &IrFunction) {
        let alloc = regalloc::allocate(func, POOL.len());
        let locals_bytes = (func.locals.len() as i32) * 8;
        let spill_bytes = (alloc.num_spill_slots as i32) * 8;
        let frame_extra = Self::round16(locals_bytes + spill_bytes);
        let epilogue = format!(".Lepilogue_{}", func.name);

        writeln!(self.out, "\n    .globl {}", func.name).unwrap();
        writeln!(self.out, "{}:", func.name).unwrap();
        writeln!(self.out, "    pushq %rbp").unwrap();
        writeln!(self.out, "    movq %rsp, %rbp").unwrap();
        for r in POOL {
            writeln!(self.out, "    pushq {}", r).unwrap();
        }
        if frame_extra > 0 {
            writeln!(self.out, "    subq ${}, %rsp", frame_extra).unwrap();
        }

        let own_label = format!("func_{}", func.name);
        for instr in &func.instructions {
            if let IrInstruction::Label(l) = instr {
                if *l == own_label {
                    continue; // symbol funkcji już wyemitowany wyżej
                }
            }
            self.compile_instr(func, &alloc, locals_bytes, instr, &epilogue);
        }

        writeln!(self.out, "{}:", epilogue).unwrap();
        if frame_extra > 0 {
            writeln!(self.out, "    addq ${}, %rsp", frame_extra).unwrap();
        }
        for r in POOL.iter().rev() {
            writeln!(self.out, "    popq {}", r).unwrap();
        }
        writeln!(self.out, "    popq %rbp").unwrap();
        writeln!(self.out, "    ret").unwrap();
    }

    fn compile_instr(
        &mut self,
        func: &IrFunction,
        alloc: &regalloc::RegAllocResult,
        locals_bytes: i32,
        instr: &IrInstruction,
        epilogue: &str,
    ) {
        match instr {
            IrInstruction::Label(l) => {
                writeln!(self.out, "{}:", Self::label(&func.name, l)).unwrap();
            }
            IrInstruction::Jump(l) => {
                writeln!(self.out, "    jmp {}", Self::label(&func.name, l)).unwrap();
            }
            IrInstruction::JumpIfZero { cond, target } => {
                let c = self.read(alloc, locals_bytes, *cond, SCRATCH1);
                writeln!(self.out, "    testq {0}, {0}", c).unwrap();
                writeln!(self.out, "    jz {}", Self::label(&func.name, target)).unwrap();
            }
            IrInstruction::JumpIfNotZero { cond, target } => {
                let c = self.read(alloc, locals_bytes, *cond, SCRATCH1);
                writeln!(self.out, "    testq {0}, {0}", c).unwrap();
                writeln!(self.out, "    jnz {}", Self::label(&func.name, target)).unwrap();
            }
            IrInstruction::LoadParam { dst, index } => {
                let arg = ARG_REGS.get(*index).unwrap_or_else(|| {
                    panic!("Xore codegen (x86_64): >6 parametrów całkowitych nie jest obsługiwane")
                });
                self.write_home(alloc, locals_bytes, *dst, arg);
            }
            IrInstruction::LoadImm { dst, value } => {
                let work = Self::work_reg(alloc, *dst);
                match value {
                    Operand::ImmInt(v) => {
                        writeln!(self.out, "    movabsq ${}, {}", v, work).unwrap();
                    }
                    Operand::ImmFloat(v) => {
                        writeln!(
                            self.out,
                            "    # UWAGA: literał zmiennoprzecinkowy {} przechowany jako surowe bity (IR nie niesie typu operacji arytmetycznej)",
                            v
                        ).unwrap();
                        writeln!(self.out, "    movabsq ${}, {}", v.to_bits() as i64, work).unwrap();
                    }
                    Operand::ImmString(s) => {
                        let clean = s.trim_matches('"');
                        let label = self.intern_string(clean);
                        writeln!(self.out, "    leaq {}(%rip), {}", label, work).unwrap();
                    }
                    Operand::Loc(_) => panic!("Operand::Loc nieoczekiwany w LoadImm"),
                }
                self.write_home(alloc, locals_bytes, *dst, work);
            }
            IrInstruction::LoadMem { dst, src } => {
                let addr = match src {
                    Location::StackSlot(off) => Self::slot_addr(*off),
                    Location::Temp(_) => panic!("Location::Temp nieoczekiwany w LoadMem"),
                };
                let work = Self::work_reg(alloc, *dst);
                writeln!(self.out, "    movq {}, {}", addr, work).unwrap();
                self.write_home(alloc, locals_bytes, *dst, work);
            }
            IrInstruction::LoadIndexed { dst, base_offset, index, elem_size } => {
                // adres = rbp - K - indeks*elem_size, gdzie K odpowiada
                // offsetowi elementu 0 (patrz slot_addr) - liczymy go w
                // dwóch krokach do %r10/%rax, żeby uniknąć trybu
                // adresowania SIB ze skalą ujemną (x86 tego nie ma).
                let idx = self.read(alloc, locals_bytes, *index, SCRATCH2);
                if idx != SCRATCH2 {
                    writeln!(self.out, "    movq {}, {}", idx, SCRATCH2).unwrap();
                }
                if *elem_size != 1 {
                    writeln!(self.out, "    imulq ${}, {}", elem_size, SCRATCH2).unwrap();
                }
                writeln!(self.out, "    leaq {}, {}", Self::slot_addr(*base_offset), SCRATCH1).unwrap();
                writeln!(self.out, "    subq {}, {}", SCRATCH2, SCRATCH1).unwrap();
                let work = Self::work_reg(alloc, *dst);
                writeln!(self.out, "    movq ({}), {}", SCRATCH1, work).unwrap();
                self.write_home(alloc, locals_bytes, *dst, work);
            }
            IrInstruction::StoreIndexed { base_offset, index, src, elem_size } => {
                let idx = self.read(alloc, locals_bytes, *index, SCRATCH2);
                if idx != SCRATCH2 {
                    writeln!(self.out, "    movq {}, {}", idx, SCRATCH2).unwrap();
                }
                if *elem_size != 1 {
                    writeln!(self.out, "    imulq ${}, {}", elem_size, SCRATCH2).unwrap();
                }
                writeln!(self.out, "    leaq {}, {}", Self::slot_addr(*base_offset), SCRATCH1).unwrap();
                writeln!(self.out, "    subq {}, {}", SCRATCH2, SCRATCH1).unwrap();
                // %r10 jest już wolny (adres policzony w %rax) - można go
                // ponownie użyć do zmaterializowania wartości źródłowej.
                let s = self.read(alloc, locals_bytes, *src, SCRATCH2);
                writeln!(self.out, "    movq {}, ({})", s, SCRATCH1).unwrap();
            }
            IrInstruction::LoadAddr { dst, base_offset } => {
                // "Rozpad" tablicy lokalnej do wskaźnika - liczy adres slotu
                // bez odczytu spod niego (odpowiednik C-owego `&arr[0]`,
                // używane przy przekazywaniu tablicy jako argumentu).
                let work = Self::work_reg(alloc, *dst);
                writeln!(self.out, "    leaq {}, {}", Self::slot_addr(*base_offset), work).unwrap();
                self.write_home(alloc, locals_bytes, *dst, work);
            }
            IrInstruction::LoadIndexedPtr { dst, base, index, elem_size } => {
                // Jak LoadIndexed, ale baza to wartość w rejestrze (wskaźnik
                // z runtime, np. parametr tablicowy), nie stały offset w
                // bieżącej ramce.
                let idx = self.read(alloc, locals_bytes, *index, SCRATCH2);
                if idx != SCRATCH2 {
                    writeln!(self.out, "    movq {}, {}", idx, SCRATCH2).unwrap();
                }
                if *elem_size != 1 {
                    writeln!(self.out, "    imulq ${}, {}", elem_size, SCRATCH2).unwrap();
                }
                let b = self.read(alloc, locals_bytes, *base, SCRATCH1);
                if b != SCRATCH1 {
                    writeln!(self.out, "    movq {}, {}", b, SCRATCH1).unwrap();
                }
                writeln!(self.out, "    subq {}, {}", SCRATCH2, SCRATCH1).unwrap();
                let work = Self::work_reg(alloc, *dst);
                writeln!(self.out, "    movq ({}), {}", SCRATCH1, work).unwrap();
                self.write_home(alloc, locals_bytes, *dst, work);
            }
            IrInstruction::StoreIndexedPtr { base, index, src, elem_size } => {
                let idx = self.read(alloc, locals_bytes, *index, SCRATCH2);
                if idx != SCRATCH2 {
                    writeln!(self.out, "    movq {}, {}", idx, SCRATCH2).unwrap();
                }
                if *elem_size != 1 {
                    writeln!(self.out, "    imulq ${}, {}", elem_size, SCRATCH2).unwrap();
                }
                let b = self.read(alloc, locals_bytes, *base, SCRATCH1);
                if b != SCRATCH1 {
                    writeln!(self.out, "    movq {}, {}", b, SCRATCH1).unwrap();
                }
                writeln!(self.out, "    subq {}, {}", SCRATCH2, SCRATCH1).unwrap();
                // %r10 jest już wolny (adres policzony w %rax) - można go
                // ponownie użyć do zmaterializowania wartości źródłowej.
                let s = self.read(alloc, locals_bytes, *src, SCRATCH2);
                writeln!(self.out, "    movq {}, ({})", s, SCRATCH1).unwrap();
            }
            IrInstruction::StoreMem { dst, src } => {
                let addr = match dst {
                    Location::StackSlot(off) => Self::slot_addr(*off),
                    Location::Temp(_) => panic!("Location::Temp nieoczekiwany w StoreMem"),
                };
                let s = self.read(alloc, locals_bytes, *src, SCRATCH1);
                writeln!(self.out, "    movq {}, {}", s, addr).unwrap();
            }
            IrInstruction::BinaryOp { dst, left, op, right } => {
                self.compile_binop(alloc, locals_bytes, *dst, *left, *op, *right);
            }
            IrInstruction::Call { dst, func: callee, args } => {
                if args.len() > ARG_REGS.len() {
                    panic!("Xore codegen (x86_64): >6 argumentów wywołania nie jest obsługiwane");
                }
                for (i, a) in args.iter().enumerate() {
                    let v = self.read(alloc, locals_bytes, *a, SCRATCH1);
                    if v != ARG_REGS[i] {
                        writeln!(self.out, "    movq {}, {}", v, ARG_REGS[i]).unwrap();
                    }
                }
                writeln!(self.out, "    call {}", callee).unwrap();
                if let Some(d) = dst {
                    self.write_home(alloc, locals_bytes, *d, "%rax");
                }
            }
            IrInstruction::Return(v) => {
                if let Some(t) = v {
                    let r = self.read(alloc, locals_bytes, *t, "%rax");
                    if r != "%rax" {
                        writeln!(self.out, "    movq {}, %rax", r).unwrap();
                    }
                } else {
                    writeln!(self.out, "    xorq %rax, %rax").unwrap();
                }
                writeln!(self.out, "    jmp {}", epilogue).unwrap();
            }
            IrInstruction::Comment(c) => {
                writeln!(self.out, "    # {}", c).unwrap();
            }
        }
    }

    fn compile_binop(
        &mut self,
        alloc: &regalloc::RegAllocResult,
        locals_bytes: i32,
        dst: Temp,
        left: Temp,
        op: IrBinOp,
        right: Temp,
    ) {
        match op {
            IrBinOp::Div | IrBinOp::Mod => {
                // idiv wymaga dzielnej w rdx:rax i dzielnika w rejestrze/pamięci
                // (nie w rdx ani rax) - materializujemy prawy operand najpierw.
                let r = self.read(alloc, locals_bytes, right, SCRATCH2);
                let moved_r = if r == "%rax" || r == "%rdx" {
                    writeln!(self.out, "    movq {}, {}", r, SCRATCH2).unwrap();
                    SCRATCH2.to_string()
                } else {
                    r
                };
                let l = self.read(alloc, locals_bytes, left, "%rax");
                if l != "%rax" {
                    writeln!(self.out, "    movq {}, %rax", l).unwrap();
                }
                writeln!(self.out, "    cqto").unwrap();
                writeln!(self.out, "    idivq {}", moved_r).unwrap();
                let result_reg = if op == IrBinOp::Div { "%rax" } else { "%rdx" };
                self.write_home(alloc, locals_bytes, dst, result_reg);
            }
            IrBinOp::DivU | IrBinOp::ModU => {
                // Jak wyżej, ale bez znaku: zamiast `cqto` (rozszerzenie ze
                // znakiem rax->rdx:rax) zerujemy rdx (rozszerzenie bez
                // znaku), i używamy `divq` zamiast `idivq`.
                let r = self.read(alloc, locals_bytes, right, SCRATCH2);
                let moved_r = if r == "%rax" || r == "%rdx" {
                    writeln!(self.out, "    movq {}, {}", r, SCRATCH2).unwrap();
                    SCRATCH2.to_string()
                } else {
                    r
                };
                let l = self.read(alloc, locals_bytes, left, "%rax");
                if l != "%rax" {
                    writeln!(self.out, "    movq {}, %rax", l).unwrap();
                }
                writeln!(self.out, "    xorq %rdx, %rdx").unwrap();
                writeln!(self.out, "    divq {}", moved_r).unwrap();
                let result_reg = if op == IrBinOp::DivU { "%rax" } else { "%rdx" };
                self.write_home(alloc, locals_bytes, dst, result_reg);
            }
            IrBinOp::Eq | IrBinOp::Neq | IrBinOp::Lt | IrBinOp::Gt | IrBinOp::Le | IrBinOp::Ge
            | IrBinOp::LtU | IrBinOp::GtU | IrBinOp::LeU | IrBinOp::GeU => {
                let l = self.read(alloc, locals_bytes, left, SCRATCH1);
                let l_reg = if l == SCRATCH1 { SCRATCH1.to_string() } else {
                    writeln!(self.out, "    movq {}, {}", l, SCRATCH1).unwrap();
                    SCRATCH1.to_string()
                };
                let r = self.read(alloc, locals_bytes, right, SCRATCH2);
                writeln!(self.out, "    cmpq {}, {}", r, l_reg).unwrap();
                let setcc = match op {
                    IrBinOp::Eq => "sete",
                    IrBinOp::Neq => "setne",
                    IrBinOp::Lt => "setl",
                    IrBinOp::Gt => "setg",
                    IrBinOp::Le => "setle",
                    IrBinOp::Ge => "setge",
                    // Warianty bez znaku: `setb`/`seta`/`setbe`/`setae`
                    // (below/above) zamiast `setl`/`setg` (less/greater) -
                    // patrzą na flagę CF zamiast SF^OF, więc dają poprawny
                    // wynik dla wartości z ustawionym najstarszym bitem.
                    IrBinOp::LtU => "setb",
                    IrBinOp::GtU => "seta",
                    IrBinOp::LeU => "setbe",
                    IrBinOp::GeU => "setae",
                    _ => unreachable!(),
                };
                writeln!(self.out, "    {} {}", setcc, Self::byte_of(SCRATCH1)).unwrap();
                writeln!(self.out, "    movzbq {}, {}", Self::byte_of(SCRATCH1), SCRATCH1).unwrap();
                self.write_home(alloc, locals_bytes, dst, SCRATCH1);
            }
            IrBinOp::And | IrBinOp::Or => {
                let l = self.read(alloc, locals_bytes, left, SCRATCH1);
                writeln!(self.out, "    testq {0}, {0}", l).unwrap();
                writeln!(self.out, "    setne {}", Self::byte_of(SCRATCH1)).unwrap();
                writeln!(self.out, "    movzbq {}, {}", Self::byte_of(SCRATCH1), SCRATCH1).unwrap();
                let r = self.read(alloc, locals_bytes, right, SCRATCH2);
                writeln!(self.out, "    testq {0}, {0}", r).unwrap();
                writeln!(self.out, "    setne {}", Self::byte_of(SCRATCH2)).unwrap();
                writeln!(self.out, "    movzbq {}, {}", Self::byte_of(SCRATCH2), SCRATCH2).unwrap();
                let inst = if op == IrBinOp::And { "andq" } else { "orq" };
                writeln!(self.out, "    {} {}, {}", inst, SCRATCH2, SCRATCH1).unwrap();
                self.write_home(alloc, locals_bytes, dst, SCRATCH1);
            }
            IrBinOp::Add | IrBinOp::Sub | IrBinOp::Mul => {
                let work = Self::work_reg(alloc, dst);
                let l = self.read(alloc, locals_bytes, left, SCRATCH1);
                if l != work {
                    writeln!(self.out, "    movq {}, {}", l, work).unwrap();
                }
                let r = self.read(alloc, locals_bytes, right, SCRATCH2);
                let inst = match op {
                    IrBinOp::Add => "addq",
                    IrBinOp::Sub => "subq",
                    IrBinOp::Mul => "imulq",
                    _ => unreachable!(),
                };
                writeln!(self.out, "    {} {}, {}", inst, r, work).unwrap();
                self.write_home(alloc, locals_bytes, dst, work);
            }
        }
    }
}
