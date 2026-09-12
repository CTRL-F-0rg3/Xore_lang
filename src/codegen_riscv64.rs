//! Generator kodu RISC-V64 (RV64GC, standardowa konwencja wywołań, GNU `as`).
//!
//! Ta sama strategia co w backendzie x86_64: tempy IR trzymamy wyłącznie w
//! rejestrach zachowywanych przez wywoływanego (s1..s8), rozłącznych z
//! rejestrami argumentów (a0..a7) i wyniku (a0), więc wywołania funkcji nie
//! wymagają rozwiązywania konfliktów kolejności przy przekazywaniu argumentów.
//!
//! Ramka stosu używa `s0` (fp) jako wskaźnika ramki, dokładnie tak jak
//! `%rbp` w backendzie x86_64 - adresy lokalne są liczone tak samo (ujemne
//! przesunięcia względem s0), co utrzymuje oba backendy symetryczne i łatwe
//! do porównania.

use crate::ir::*;
use crate::regalloc::{self, Home};
use std::collections::HashMap;
use std::fmt::Write as _;

const POOL: [&str; 8] = ["s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8"];
const ARG_REGS: [&str; 8] = ["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"];
const SCRATCH1: &str = "t0";
const SCRATCH2: &str = "t1";
// miejsce na ra (8) + s0 (8) + rejestry puli
const HEADER_BYTES: i32 = 16 + (POOL.len() as i32) * 8;

pub struct RiscV64CodeGen {
    out: String,
    strings: Vec<String>,
    string_labels: HashMap<String, String>,
}

impl RiscV64CodeGen {
    pub fn new() -> Self {
        Self { out: String::new(), strings: Vec::new(), string_labels: HashMap::new() }
    }

    pub fn compile(mut self, program: &IrProgram) -> String {
        writeln!(self.out, "    .text").unwrap();

        // Punkt wejścia programu: tak jak w backendzie x86_64 - Xore jest
        // zawsze budowany jako plik freestanding (bez libc/crt0). `_start`
        // woła `main`, a jej wynik (a0) trafia bezpośrednio do syscalla exit.
        writeln!(self.out, "    .globl _start").unwrap();
        writeln!(self.out, "_start:").unwrap();
        writeln!(self.out, "    call main").unwrap();
        writeln!(self.out, "    li a7, 93    # sys_exit").unwrap();
        writeln!(self.out, "    ecall").unwrap();

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
        format!("-{}(s0)", HEADER_BYTES + (-off))
    }

    fn spill_addr(locals_bytes: i32, k: usize) -> String {
        format!("-{}(s0)", HEADER_BYTES + locals_bytes + 8 * (k as i32 + 1))
    }

    /// Mnoży rejestr `reg` przez `scale` w miejscu - `slli` dla potęg
    /// dwójki (jedyny przypadek, jaki dziś realnie występuje, bo wszystkie
    /// elementy zajmują 8 bajtów), z bezpiecznym fallbackiem na `li`+`mul`
    /// dla innych wartości (na przyszłość, gdyby rozmiar elementu się zmienił).
    fn emit_scale(out: &mut String, reg: &str, scale: i32) {
        if scale == 1 {
            return;
        }
        if scale > 0 && (scale & (scale - 1)) == 0 {
            let shift = scale.trailing_zeros();
            writeln!(out, "    slli {}, {}, {}", reg, reg, shift).unwrap();
        } else {
            writeln!(out, "    li t3, {}", scale).unwrap();
            writeln!(out, "    mul {}, {}, t3", reg, reg).unwrap();
        }
    }

    fn label(func_name: &str, l: &str) -> String {
        format!(".L{}_{}", func_name, l)
    }

    fn read(&mut self, alloc: &regalloc::RegAllocResult, locals_bytes: i32, t: Temp, scratch: &str) -> String {
        match alloc.home[&t] {
            Home::Reg(r) => POOL[r].to_string(),
            Home::Spill(k) => {
                writeln!(self.out, "    ld {}, {}", scratch, Self::spill_addr(locals_bytes, k)).unwrap();
                scratch.to_string()
            }
        }
    }

    fn write_home(&mut self, alloc: &regalloc::RegAllocResult, locals_bytes: i32, t: Temp, src_reg: &str) {
        match alloc.home[&t] {
            Home::Reg(r) => {
                if POOL[r] != src_reg {
                    writeln!(self.out, "    mv {}, {}", POOL[r], src_reg).unwrap();
                }
            }
            Home::Spill(k) => {
                writeln!(self.out, "    sd {}, {}", src_reg, Self::spill_addr(locals_bytes, k)).unwrap();
            }
        }
    }

    fn work_reg(alloc: &regalloc::RegAllocResult, t: Temp) -> &'static str {
        match alloc.home[&t] {
            Home::Reg(r) => POOL[r],
            Home::Spill(_) => SCRATCH1,
        }
    }

    fn compile_function(&mut self, func: &IrFunction) {
        let alloc = regalloc::allocate(func, POOL.len());
        let locals_bytes = (func.locals.len() as i32) * 8;
        let spill_bytes = (alloc.num_spill_slots as i32) * 8;
        let frame = Self::round16(HEADER_BYTES + locals_bytes + spill_bytes);
        let epilogue = format!(".Lepilogue_{}", func.name);

        writeln!(self.out, "\n    .globl {}", func.name).unwrap();
        writeln!(self.out, "{}:", func.name).unwrap();
        writeln!(self.out, "    addi sp, sp, -{}", frame).unwrap();
        writeln!(self.out, "    sd ra, {}(sp)", frame - 8).unwrap();
        writeln!(self.out, "    sd s0, {}(sp)", frame - 16).unwrap();
        writeln!(self.out, "    addi s0, sp, {}", frame).unwrap();
        for (i, r) in POOL.iter().enumerate() {
            writeln!(self.out, "    sd {}, -{}(s0)", r, 16 + 8 * (i as i32 + 1)).unwrap();
        }

        let own_label = format!("func_{}", func.name);
        for instr in &func.instructions {
            if let IrInstruction::Label(l) = instr {
                if *l == own_label {
                    continue;
                }
            }
            self.compile_instr(func, &alloc, locals_bytes, instr, &epilogue);
        }

        writeln!(self.out, "{}:", epilogue).unwrap();
        for (i, r) in POOL.iter().enumerate() {
            writeln!(self.out, "    ld {}, -{}(s0)", r, 16 + 8 * (i as i32 + 1)).unwrap();
        }
        writeln!(self.out, "    ld ra, {}(sp)", frame - 8).unwrap();
        writeln!(self.out, "    ld s0, {}(sp)", frame - 16).unwrap();
        writeln!(self.out, "    addi sp, sp, {}", frame).unwrap();
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
                writeln!(self.out, "    j {}", Self::label(&func.name, l)).unwrap();
            }
            IrInstruction::JumpIfZero { cond, target } => {
                let c = self.read(alloc, locals_bytes, *cond, SCRATCH1);
                writeln!(self.out, "    beqz {}, {}", c, Self::label(&func.name, target)).unwrap();
            }
            IrInstruction::JumpIfNotZero { cond, target } => {
                let c = self.read(alloc, locals_bytes, *cond, SCRATCH1);
                writeln!(self.out, "    bnez {}, {}", c, Self::label(&func.name, target)).unwrap();
            }
            IrInstruction::LoadParam { dst, index } => {
                let arg = ARG_REGS.get(*index).unwrap_or_else(|| {
                    panic!("Xore codegen (riscv64): >8 parametrów całkowitych nie jest obsługiwane")
                });
                self.write_home(alloc, locals_bytes, *dst, arg);
            }
            IrInstruction::LoadImm { dst, value } => {
                let work = Self::work_reg(alloc, *dst);
                match value {
                    Operand::ImmInt(v) => {
                        writeln!(self.out, "    li {}, {}", work, v).unwrap();
                    }
                    Operand::ImmFloat(v) => {
                        writeln!(
                            self.out,
                            "    # UWAGA: literał zmiennoprzecinkowy {} przechowany jako surowe bity (IR nie niesie typu operacji arytmetycznej)",
                            v
                        ).unwrap();
                        writeln!(self.out, "    li {}, {}", work, v.to_bits() as i64).unwrap();
                    }
                    Operand::ImmString(s) => {
                        let clean = s.trim_matches('"');
                        let label = self.intern_string(clean);
                        writeln!(self.out, "    la {}, {}", work, label).unwrap();
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
                writeln!(self.out, "    ld {}, {}", work, addr).unwrap();
                self.write_home(alloc, locals_bytes, *dst, work);
            }
            IrInstruction::LoadIndexed { dst, base_offset, index, elem_size } => {
                // adres = s0 - K - indeks*elem_size (K = przesunięcie
                // elementu 0, patrz slot_addr). Liczone w dwóch krokach do
                // rejestru t2, bo RISC-V nie ma trybów adresowania
                // skalowanego jak x86 SIB.
                let idx = self.read(alloc, locals_bytes, *index, SCRATCH2);
                if idx != SCRATCH2 {
                    writeln!(self.out, "    mv {}, {}", SCRATCH2, idx).unwrap();
                }
                Self::emit_scale(&mut self.out, SCRATCH2, *elem_size);
                writeln!(self.out, "    li t2, {}", HEADER_BYTES + (-*base_offset)).unwrap();
                writeln!(self.out, "    sub t2, s0, t2").unwrap();
                writeln!(self.out, "    sub t2, t2, {}", SCRATCH2).unwrap();
                let work = Self::work_reg(alloc, *dst);
                writeln!(self.out, "    ld {}, 0(t2)", work).unwrap();
                self.write_home(alloc, locals_bytes, *dst, work);
            }
            IrInstruction::StoreIndexed { base_offset, index, src, elem_size } => {
                let idx = self.read(alloc, locals_bytes, *index, SCRATCH2);
                if idx != SCRATCH2 {
                    writeln!(self.out, "    mv {}, {}", SCRATCH2, idx).unwrap();
                }
                Self::emit_scale(&mut self.out, SCRATCH2, *elem_size);
                writeln!(self.out, "    li t2, {}", HEADER_BYTES + (-*base_offset)).unwrap();
                writeln!(self.out, "    sub t2, s0, t2").unwrap();
                writeln!(self.out, "    sub t2, t2, {}", SCRATCH2).unwrap();
                // t1 jest już wolny (adres policzony w t2) - można go
                // ponownie użyć do zmaterializowania wartości źródłowej.
                let s = self.read(alloc, locals_bytes, *src, SCRATCH2);
                writeln!(self.out, "    sd {}, 0(t2)", s).unwrap();
            }
            IrInstruction::StoreMem { dst, src } => {
                let addr = match dst {
                    Location::StackSlot(off) => Self::slot_addr(*off),
                    Location::Temp(_) => panic!("Location::Temp nieoczekiwany w StoreMem"),
                };
                let s = self.read(alloc, locals_bytes, *src, SCRATCH1);
                writeln!(self.out, "    sd {}, {}", s, addr).unwrap();
            }
            IrInstruction::BinaryOp { dst, left, op, right } => {
                self.compile_binop(alloc, locals_bytes, *dst, *left, *op, *right);
            }
            IrInstruction::Call { dst, func: callee, args } => {
                if args.len() > ARG_REGS.len() {
                    panic!("Xore codegen (riscv64): >8 argumentów wywołania nie jest obsługiwane");
                }
                for (i, a) in args.iter().enumerate() {
                    let v = self.read(alloc, locals_bytes, *a, SCRATCH1);
                    if v != ARG_REGS[i] {
                        writeln!(self.out, "    mv {}, {}", ARG_REGS[i], v).unwrap();
                    }
                }
                writeln!(self.out, "    call {}", callee).unwrap();
                if let Some(d) = dst {
                    self.write_home(alloc, locals_bytes, *d, "a0");
                }
            }
            IrInstruction::Return(v) => {
                if let Some(t) = v {
                    let r = self.read(alloc, locals_bytes, *t, "a0");
                    if r != "a0" {
                        writeln!(self.out, "    mv a0, {}", r).unwrap();
                    }
                } else {
                    writeln!(self.out, "    li a0, 0").unwrap();
                }
                writeln!(self.out, "    j {}", epilogue).unwrap();
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
        let work = Self::work_reg(alloc, dst);
        let l = self.read(alloc, locals_bytes, left, SCRATCH1);
        let r = self.read(alloc, locals_bytes, right, SCRATCH2);
        match op {
            IrBinOp::Add => { writeln!(self.out, "    add {}, {}, {}", work, l, r).unwrap(); }
            IrBinOp::Sub => { writeln!(self.out, "    sub {}, {}, {}", work, l, r).unwrap(); }
            IrBinOp::Mul => { writeln!(self.out, "    mul {}, {}, {}", work, l, r).unwrap(); }
            IrBinOp::Div => { writeln!(self.out, "    div {}, {}, {}", work, l, r).unwrap(); }
            IrBinOp::Mod => { writeln!(self.out, "    rem {}, {}, {}", work, l, r).unwrap(); }
            IrBinOp::Eq => {
                writeln!(self.out, "    sub {}, {}, {}", work, l, r).unwrap();
                writeln!(self.out, "    seqz {}, {}", work, work).unwrap();
            }
            IrBinOp::Neq => {
                writeln!(self.out, "    sub {}, {}, {}", work, l, r).unwrap();
                writeln!(self.out, "    snez {}, {}", work, work).unwrap();
            }
            IrBinOp::Lt => { writeln!(self.out, "    slt {}, {}, {}", work, l, r).unwrap(); }
            IrBinOp::Gt => { writeln!(self.out, "    slt {}, {}, {}", work, r, l).unwrap(); }
            IrBinOp::Le => {
                writeln!(self.out, "    slt {}, {}, {}", work, r, l).unwrap();
                writeln!(self.out, "    xori {}, {}, 1", work, work).unwrap();
            }
            IrBinOp::Ge => {
                writeln!(self.out, "    slt {}, {}, {}", work, l, r).unwrap();
                writeln!(self.out, "    xori {}, {}, 1", work, work).unwrap();
            }
            IrBinOp::And => {
                writeln!(self.out, "    snez {}, {}", SCRATCH1, l).unwrap();
                writeln!(self.out, "    snez {}, {}", SCRATCH2, r).unwrap();
                writeln!(self.out, "    and {}, {}, {}", work, SCRATCH1, SCRATCH2).unwrap();
            }
            IrBinOp::Or => {
                writeln!(self.out, "    or {}, {}, {}", work, l, r).unwrap();
                writeln!(self.out, "    snez {}, {}", work, work).unwrap();
            }
        }
        self.write_home(alloc, locals_bytes, dst, work);
    }
}
