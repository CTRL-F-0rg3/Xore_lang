//! Alokacja rejestrów typu "linear scan" (Poletto-Sarkar), niezależna od
//! architektury.
//!
//! Kluczowa decyzja projektowa: tempy IR alokujemy WYŁĄCZNIE do rejestrów
//! zachowywanych przez wywoływanego (callee-saved). Dzięki temu w miejscu
//! wywołania funkcji (`Call`) nigdy nie trzeba martwić się o to, że rejestr
//! trzymający żywą wartość zostanie nadpisany przez wywoływaną funkcję ani
//! o kolizję z rejestrami przenoszącymi argumenty (te są caller-saved) -
//! oba zbiory rejestrów są rozłączne, więc przekazywanie argumentów to
//! zawsze proste, niezależne od kolejności `mov`y, bez potrzeby rozwiązywania
//! "równoległych podstawień" (parallel copy problem).
//!
//! Gdy tempów żywych jednocześnie jest więcej niż dostępnych rejestrów,
//! nadwyżka jest rozlewana (spill) do dodatkowych slotów na stosie funkcji,
//! wybierając do rozlania interwał kończący się najpóźniej (klasyczna
//! heurystyka linear-scan - minimalizuje liczbę przyszłych konfliktów).

use crate::ir::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// Indeks w puli rejestrów callee-saved danej architektury (0-based).
    Reg(usize),
    /// Indeks dodatkowego slotu na stosie zarezerwowanego na rozlaną wartość.
    Spill(usize),
}

pub struct RegAllocResult {
    pub home: HashMap<Temp, Home>,
    pub num_spill_slots: usize,
}

struct Interval {
    temp: Temp,
    start: usize,
    end: usize,
}

/// Wylicza dla każdego tempa jego przedział życia w obrębie funkcji,
/// przybliżony liniowym porządkiem instrukcji (bez pętli w Xore i bez
/// wielokrotnego definiowania tego samego tempa, ta aproksymacja jest
/// zawsze bezpieczna - co najwyżej sztucznie wydłuża interwał w gałęzi,
/// która i tak nie zostanie wykonana).
fn compute_intervals(func: &IrFunction) -> Vec<Interval> {
    let mut def: HashMap<Temp, usize> = HashMap::new();
    let mut last_use: HashMap<Temp, usize> = HashMap::new();

    let mut note_def = |t: Temp, i: usize, def: &mut HashMap<Temp, usize>| {
        def.entry(t).or_insert(i);
    };
    let mut note_use = |t: Temp, i: usize, last_use: &mut HashMap<Temp, usize>| {
        last_use.insert(t, i);
    };

    for (i, instr) in func.instructions.iter().enumerate() {
        match instr {
            IrInstruction::LoadParam { dst, .. } => note_def(*dst, i, &mut def),
            IrInstruction::LoadImm { dst, .. } => note_def(*dst, i, &mut def),
            IrInstruction::LoadMem { dst, .. } => note_def(*dst, i, &mut def),
            IrInstruction::LoadIndexed { dst, index, .. } => {
                note_def(*dst, i, &mut def);
                note_use(*index, i, &mut last_use);
            }
            IrInstruction::StoreIndexed { index, src, .. } => {
                note_use(*index, i, &mut last_use);
                note_use(*src, i, &mut last_use);
            }
            IrInstruction::LoadAddr { dst, .. } => note_def(*dst, i, &mut def),
            IrInstruction::LoadIndexedPtr { dst, base, index, .. } => {
                note_def(*dst, i, &mut def);
                note_use(*base, i, &mut last_use);
                note_use(*index, i, &mut last_use);
            }
            IrInstruction::StoreIndexedPtr { base, index, src, .. } => {
                note_use(*base, i, &mut last_use);
                note_use(*index, i, &mut last_use);
                note_use(*src, i, &mut last_use);
            }
            IrInstruction::BinaryOp { dst, left, right, .. } => {
                note_def(*dst, i, &mut def);
                note_use(*left, i, &mut last_use);
                note_use(*right, i, &mut last_use);
            }
            IrInstruction::StoreMem { src, .. } => note_use(*src, i, &mut last_use),
            IrInstruction::Call { dst, args, .. } => {
                if let Some(d) = dst {
                    note_def(*d, i, &mut def);
                }
                for a in args {
                    note_use(*a, i, &mut last_use);
                }
            }
            IrInstruction::JumpIfZero { cond, .. } | IrInstruction::JumpIfNotZero { cond, .. } => {
                note_use(*cond, i, &mut last_use);
            }
            IrInstruction::Return(Some(v)) => note_use(*v, i, &mut last_use),
            _ => {}
        }
    }

    let mut intervals = Vec::new();
    for (&temp, &start) in &def {
        let end = *last_use.get(&temp).unwrap_or(&start);
        intervals.push(Interval { temp, start, end: end.max(start) });
    }
    intervals.sort_by_key(|iv| iv.start);
    intervals
}

pub fn allocate(func: &IrFunction, num_registers: usize) -> RegAllocResult {
    let intervals = compute_intervals(func);

    let mut home: HashMap<Temp, Home> = HashMap::new();
    let mut free_regs: Vec<usize> = (0..num_registers).rev().collect();
    // aktywne interwały: (koniec, temp, rejestr)
    let mut active: Vec<(usize, Temp, usize)> = Vec::new();
    let mut num_spill_slots = 0usize;

    for iv in &intervals {
        // Tempy zmiennoprzecinkowe NIGDY nie trafiają do puli rejestrów
        // ogólnego przeznaczenia - zawsze rezydują w pamięci (patrz komentarz
        // przy `IrFunction::float_temps`: x86_64 SysV nie ma callee-saved
        // rejestrów xmm, więc nasza strategia "tylko callee-saved" nie da
        // się zastosować do floatów; upraszczamy, zamiast budować drugi,
        // równoległy alokator dla osobnej klasy rejestrów).
        if func.float_temps.contains(&iv.temp) {
            home.insert(iv.temp, Home::Spill(num_spill_slots));
            num_spill_slots += 1;
            continue;
        }

        // Zwolnij rejestry interwałów, które już się skończyły.
        active.retain(|(end, t, reg)| {
            if *end < iv.start {
                free_regs.push(*reg);
                let _ = t;
                false
            } else {
                true
            }
        });

        if let Some(reg) = free_regs.pop() {
            home.insert(iv.temp, Home::Reg(reg));
            active.push((iv.end, iv.temp, reg));
            active.sort_by_key(|(end, _, _)| *end);
        } else {
            // Brak wolnego rejestru: rozlewamy interwał kończący się najpóźniej
            // spośród aktywnych ORAZ bieżącego (klasyczna heurystyka linear-scan).
            if let Some(&(furthest_end, furthest_temp, reg)) = active.last() {
                if furthest_end > iv.end {
                    // Rozlej dotychczasowego posiadacza rejestru, przydziel
                    // zwolniony rejestr bieżącemu (krótszemu) interwałowi.
                    home.insert(furthest_temp, Home::Spill(num_spill_slots));
                    num_spill_slots += 1;
                    active.pop();
                    home.insert(iv.temp, Home::Reg(reg));
                    active.push((iv.end, iv.temp, reg));
                    active.sort_by_key(|(end, _, _)| *end);
                    continue;
                }
            }
            home.insert(iv.temp, Home::Spill(num_spill_slots));
            num_spill_slots += 1;
        }
    }

    RegAllocResult { home, num_spill_slots }
}
