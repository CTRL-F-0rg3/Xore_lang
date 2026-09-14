use crate::ast::Type;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Temp(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    Temp(Temp),
    StackSlot(i32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Loc(Location),
    ImmInt(i64),
    ImmFloat(f64),
    ImmString(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrInstruction {
    /// Pobiera wartość i-tego parametru wejściowego funkcji (przekazanego zgodnie
    /// z konwencją wywołań danej architektury) i umieszcza go w rejestrze wirtualnym.
    /// Bez tej instrukcji parametry funkcji byłyby nieinicjalizowane w kodzie maszynowym
    /// - był to brakujący element oryginalnego `lowering.rs`.
    LoadParam { dst: Temp, index: usize },
    LoadImm { dst: Temp, value: Operand },
    LoadMem { dst: Temp, src: Location },
    StoreMem { dst: Location, src: Temp },
    /// Odczyt elementu tablicy pod dynamicznym (nieznanym w czasie
    /// kompilacji) indeksem: `dst = *(ramka + base_offset - index*elem_size)`.
    /// Dla stałych indeksów `lowering.rs` generuje zwykły `LoadMem` zamiast
    /// tego (korzysta wtedy z istniejących optymalizacji store->load).
    LoadIndexed { dst: Temp, base_offset: i32, index: Temp, elem_size: i32 },
    /// Zapis do elementu tablicy pod dynamicznym indeksem - symetrycznie do
    /// `LoadIndexed`.
    StoreIndexed { base_offset: i32, index: Temp, src: Temp, elem_size: i32 },
    /// Liczy adres slotu na stosie (`ramka + base_offset`) i wkłada go do
    /// rejestru jako zwykłą wartość (bez odczytu spod tego adresu). Używane,
    /// gdy tablica lokalna "rozpada się" do wskaźnika (np. przekazana jako
    /// argument wywołania funkcji - dokładnie jak w C).
    LoadAddr { dst: Temp, base_offset: i32 },
    /// Jak `LoadIndexed`, ale baza adresu jest wartością w rejestrze
    /// (wskaźnikiem otrzymanym w runtime, np. jako parametr funkcji), a nie
    /// stałym przesunięciem w bieżącej ramce stosu: `dst = *(base - index*elem_size)`.
    LoadIndexedPtr { dst: Temp, base: Temp, index: Temp, elem_size: i32 },
    /// Zapis symetryczny do `LoadIndexedPtr`.
    StoreIndexedPtr { base: Temp, index: Temp, src: Temp, elem_size: i32 },
    BinaryOp { dst: Temp, left: Temp, op: IrBinOp, right: Temp },
    Call { dst: Option<Temp>, func: String, args: Vec<Temp> },
    Label(String),
    Jump(String),
    JumpIfZero { cond: Temp, target: String },
    JumpIfNotZero { cond: Temp, target: String },
    Return(Option<Temp>),
    Comment(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrBinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Neq, Lt, Gt, Le, Ge,
    /// Warianty bez znaku (dla `u32`/`u64`) - dają inny wynik niż warianty
    /// ze znakiem dla wartości z ustawionym najstarszym bitem (np.
    /// porównanie czy dzielenie liczb bliskich `u32::MAX`). Wybierane w
    /// `lowering.rs` na podstawie wywnioskowanego typu operandów.
    DivU, ModU, LtU, GtU, LeU, GeU,
    And, Or,
}

#[derive(Debug, Clone)]
pub struct IrFunction {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub return_type: Option<Type>,
    pub locals: Vec<(String, Type)>,
    pub instructions: Vec<IrInstruction>,
    pub next_temp: u32,
}

impl IrFunction {
    pub fn new_temp(&mut self) -> Temp {
        let t = Temp(self.next_temp);
        self.next_temp += 1;
        t
    }
}

#[derive(Debug, Clone)]
pub struct IrProgram {
    pub functions: Vec<IrFunction>,
}

impl fmt::Display for IrProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for func in &self.functions {
            writeln!(f, "function {}({:?}) -> {:?}", func.name, func.params, func.return_type)?;
            writeln!(f, "  locals: {:?}", func.locals)?;
            for instr in &func.instructions {
                writeln!(f, "    {}", format_instruction(instr))?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

fn format_instruction(instr: &IrInstruction) -> String {
    match instr {
        IrInstruction::LoadParam { dst, index } => format!("{:?} = param#{}", dst, index),
        IrInstruction::LoadImm { dst, value } => format!("{:?} = {:?}", dst, value),
        IrInstruction::LoadMem { dst, src } => format!("{:?} = load {:?}", dst, src),
        IrInstruction::StoreMem { dst, src } => format!("store {:?}, {:?}", dst, src),
        IrInstruction::LoadIndexed { dst, base_offset, index, elem_size } => {
            format!("{:?} = load [base{}, {:?}*{}]", dst, base_offset, index, elem_size)
        }
        IrInstruction::StoreIndexed { base_offset, index, src, elem_size } => {
            format!("store [base{}, {:?}*{}], {:?}", base_offset, index, elem_size, src)
        }
        IrInstruction::LoadAddr { dst, base_offset } => format!("{:?} = addr_of(base{})", dst, base_offset),
        IrInstruction::LoadIndexedPtr { dst, base, index, elem_size } => {
            format!("{:?} = load [*{:?}, {:?}*{}]", dst, base, index, elem_size)
        }
        IrInstruction::StoreIndexedPtr { base, index, src, elem_size } => {
            format!("store [*{:?}, {:?}*{}], {:?}", base, index, elem_size, src)
        }
        IrInstruction::BinaryOp { dst, left, op, right } => format!("{:?} = {:?} {:?} {:?}", dst, left, op, right),
        IrInstruction::Call { dst, func, args } => {
            let dst_str = dst.map(|t| format!("{:?} = ", t)).unwrap_or_default();
            format!("{}call {}({:?})", dst_str, func, args)
        }
        IrInstruction::Label(name) => format!("{}:", name),
        IrInstruction::Jump(target) => format!("goto {}", target),
        IrInstruction::JumpIfZero { cond, target } => format!("if_zero {:?} goto {}", cond, target),
        IrInstruction::JumpIfNotZero { cond, target } => format!("if_not_zero {:?} goto {}", cond, target),
        IrInstruction::Return(val) => format!("return {:?}", val),
        IrInstruction::Comment(c) => format!("; {}", c),
    }
}
