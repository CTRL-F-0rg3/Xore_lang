//! Optymalizacje na poziomie IR (przed alokacją rejestrów i generacją kodu).
//!
//! Xore celowo nie ma pętli i nie ma operatora adresu/wskaźnika dostępnego
//! z poziomu składni, więc zmienne lokalne (StackSlot) nigdy nie "uciekają"
//! poza funkcję. To pozwala na bezpieczne, proste analizy w obrębie jednej
//! funkcji bez martwienia się o aliasing między funkcjami.
//!
//! Wykonywane przebiegi (w pętli aż do punktu stałego):
//!   1. Stałe składanie (constant folding) dla operacji arytmetycznych i porównań.
//!   2. Upraszczanie skoków warunkowych o stałym warunku (branch folding).
//!   3. Przekazywanie store->load ("mem2reg" w małej skali) + eliminacja
//!      martwych zapisów do zmiennych lokalnych.
//!   4. Eliminacja martwego kodu (DCE) dla tempów, które nigdy nie są użyte.
//!   5. Peephole: usuwanie skoków-do-następnej-etykiety i kodu nieosiągalnego.

use crate::ir::*;
use std::collections::HashMap;

pub fn optimize_program(program: &mut IrProgram) {
    for func in &mut program.functions {
        optimize_function(func);
    }
}

pub fn optimize_function(func: &mut IrFunction) {
    loop {
        let mut changed = false;
        changed |= constant_fold(func);
        changed |= fold_branches(func);
        changed |= mem_forward(func);
        changed |= dead_code_elim(func);
        changed |= peephole(func);
        if !changed {
            break;
        }
    }
}

fn as_int(op: &Operand) -> Option<i64> {
    match op {
        Operand::ImmInt(v) => Some(*v),
        _ => None,
    }
}

/// Stałe składanie: jeśli oba argumenty operacji binarnej to znane stałe
/// (bezpośrednio poprzedzone przez LoadImm i nigdy więcej nie nadpisane -
/// co zawsze jest prawdą, bo każdy Temp jest przypisywany dokładnie raz),
/// zastępujemy operację jedną instrukcją LoadImm.
fn constant_fold(func: &mut IrFunction) -> bool {
    let mut consts: HashMap<Temp, i64> = HashMap::new();
    let mut changed = false;

    for i in 0..func.instructions.len() {
        let replacement = match &func.instructions[i] {
            IrInstruction::LoadImm { dst, value } => {
                if let Some(v) = as_int(value) {
                    consts.insert(*dst, v);
                }
                None
            }
            IrInstruction::BinaryOp { dst, left, op, right } => {
                if let (Some(&lv), Some(&rv)) = (consts.get(left), consts.get(right)) {
                    fold_binop(*op, lv, rv).map(|result| (*dst, result))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some((dst, result)) = replacement {
            func.instructions[i] = IrInstruction::LoadImm { dst, value: Operand::ImmInt(result) };
            consts.insert(dst, result);
            changed = true;
        }
    }
    changed
}

fn fold_binop(op: IrBinOp, l: i64, r: i64) -> Option<i64> {
    // Warianty float (`F*`) celowo nie są tu składane: `consts` operuje na
    // `i64` (bity), a stałe składanie floatów wymagałoby osobnej ścieżki
    // interpretującej te bity jako `f64` - nieobsłużone w tej rundzie
    // (bezpiecznie: po prostu nie optymalizujemy, nie liczymy źle).
    if matches!(op, IrBinOp::FAdd | IrBinOp::FSub | IrBinOp::FMul | IrBinOp::FDiv
        | IrBinOp::FEq | IrBinOp::FNeq | IrBinOp::FLt | IrBinOp::FGt | IrBinOp::FLe | IrBinOp::FGe)
    {
        return None;
    }
    Some(match op {
        IrBinOp::Add => l.wrapping_add(r),
        IrBinOp::Sub => l.wrapping_sub(r),
        IrBinOp::Mul => l.wrapping_mul(r),
        IrBinOp::Div => {
            if r == 0 {
                return None;
            }
            l.wrapping_div(r)
        }
        IrBinOp::Mod => {
            if r == 0 {
                return None;
            }
            l.wrapping_rem(r)
        }
        IrBinOp::Eq => (l == r) as i64,
        IrBinOp::Neq => (l != r) as i64,
        IrBinOp::Lt => (l < r) as i64,
        IrBinOp::Gt => (l > r) as i64,
        IrBinOp::Le => (l <= r) as i64,
        IrBinOp::Ge => (l >= r) as i64,
        // Warianty bez znaku: reinterpretujemy oba operandy jako u64 przed
        // porównaniem/dzieleniem - to jedyna różnica względem wariantów ze
        // znakiem wyżej.
        IrBinOp::DivU => {
            if r == 0 {
                return None;
            }
            (l as u64).wrapping_div(r as u64) as i64
        }
        IrBinOp::ModU => {
            if r == 0 {
                return None;
            }
            (l as u64).wrapping_rem(r as u64) as i64
        }
        IrBinOp::LtU => ((l as u64) < (r as u64)) as i64,
        IrBinOp::GtU => ((l as u64) > (r as u64)) as i64,
        IrBinOp::LeU => ((l as u64) <= (r as u64)) as i64,
        IrBinOp::GeU => ((l as u64) >= (r as u64)) as i64,
        IrBinOp::And => ((l != 0) && (r != 0)) as i64,
        IrBinOp::Or => ((l != 0) || (r != 0)) as i64,
        // Nieosiągalne - odfiltrowane wczesnym `return None` na górze
        // funkcji, ale `match` i tak musi być wyczerpujący.
        IrBinOp::FAdd | IrBinOp::FSub | IrBinOp::FMul | IrBinOp::FDiv
        | IrBinOp::FEq | IrBinOp::FNeq | IrBinOp::FLt | IrBinOp::FGt | IrBinOp::FLe | IrBinOp::FGe => {
            unreachable!("warianty float odfiltrowane wcześniej")
        }
    })
}

/// Jeżeli warunek skoku warunkowego jest znaną stałą, zamienia go na
/// bezwarunkowy `Jump` albo usuwa całkowicie (fall-through).
fn fold_branches(func: &mut IrFunction) -> bool {
    let mut consts: HashMap<Temp, i64> = HashMap::new();
    let mut changed = false;

    for i in 0..func.instructions.len() {
        match &func.instructions[i] {
            IrInstruction::LoadImm { dst, value } => {
                if let Some(v) = as_int(value) {
                    consts.insert(*dst, v);
                }
            }
            IrInstruction::JumpIfZero { cond, target } => {
                if let Some(&v) = consts.get(cond) {
                    func.instructions[i] = if v == 0 {
                        IrInstruction::Jump(target.clone())
                    } else {
                        IrInstruction::Comment(format!("usunięto martwy skok do {}", target))
                    };
                    changed = true;
                }
            }
            IrInstruction::JumpIfNotZero { cond, target } => {
                if let Some(&v) = consts.get(cond) {
                    func.instructions[i] = if v != 0 {
                        IrInstruction::Jump(target.clone())
                    } else {
                        IrInstruction::Comment(format!("usunięto martwy skok do {}", target))
                    };
                    changed = true;
                }
            }
            _ => {}
        }
    }
    changed
}

/// "Mem2reg" w małej skali: śledzi ostatnią wartość zapisaną do każdego
/// slotu na stosie. Odczyt (LoadMem) tuż po zapisie tej samej wartości jest
/// zastępowany bezpośrednim użyciem tempa źródłowego zamiast realnego
/// odczytu z pamięci. Zapis, który jest nadpisany innym zapisem zanim
/// ktokolwiek go odczyta, jest martwy i zostaje usunięty.
///
/// Dla bezpieczeństwa mapowanie jest czyszczone przy każdej etykiecie
/// (potencjalny cel skoku z innej ścieżki) - to konserwatywne, ale zawsze
/// poprawne podejście bez pełnej analizy przepływu sterowania.
fn mem_forward(func: &mut IrFunction) -> bool {
    // Jeśli funkcja bierze adres jakiegokolwiek slotu na stosie
    // (`LoadAddr` - dzieje się to, gdy tablica lokalna "rozpada się" do
    // wskaźnika, np. przekazana jako argument wywołania), to jej pamięć
    // może być czytana ALBO ZAPISYWANA przez wywołaną funkcję poprzez ten
    // wskaźnik (patrz `StoreIndexedPtr`) - czyli w sposób NIEWIDOCZNY dla
    // tej analizy (żaden `LoadMem`/`StoreMem` w tej funkcji tego nie pokazuje).
    // Bez tego zabezpieczenia:
    //   - "martwe zapisy" inicjalizujące tablicę zostałyby usunięte, bo
    //     nigdy nie są odczytane LOKALNIE (tylko przez wywołaną funkcję) -
    //     to realny bug znaleziony przy testowaniu (tablica przekazana do
    //     funkcji wracała z samymi zerami),
    //   - odczyt po wywołaniu funkcji mógłby dostać przeterminowaną,
    //     podstawioną wartość sprzed wywołania, jeśli wywołana funkcja
    //     zmodyfikowała tablicę przez wskaźnik.
    // Konserwatywne, zawsze poprawne rozwiązanie: dla takich funkcji w
    // ogóle wyłączamy ten przebieg.
    if func.instructions.iter().any(|i| matches!(i, IrInstruction::LoadAddr { .. })) {
        return false;
    }

    let mut changed = false;
    let n = func.instructions.len();

    // alias[t] = t' oznacza "użycie t należy zastąpić przez t'"
    let mut alias: HashMap<Temp, Temp> = HashMap::new();
    // ostatnia wartość zapisana pod danym adresem, oraz czy była odczytana
    let mut last_store: HashMap<Location, (usize, Temp, bool)> = HashMap::new();
    let mut to_remove: Vec<usize> = Vec::new();

    let resolve = |alias: &HashMap<Temp, Temp>, t: Temp| -> Temp {
        let mut cur = t;
        while let Some(&next) = alias.get(&cur) {
            if next == cur { break; }
            cur = next;
        }
        cur
    };

    for i in 0..n {
        // Najpierw podmień operandy zgodnie z dotychczasowymi aliasami.
        match &mut func.instructions[i] {
            IrInstruction::BinaryOp { left, right, .. } => {
                *left = resolve(&alias, *left);
                *right = resolve(&alias, *right);
            }
            IrInstruction::StoreMem { src, .. } => {
                *src = resolve(&alias, *src);
            }
            IrInstruction::LoadIndexed { index, .. } => {
                *index = resolve(&alias, *index);
            }
            IrInstruction::StoreIndexed { index, src, .. } => {
                *index = resolve(&alias, *index);
                *src = resolve(&alias, *src);
            }
            IrInstruction::LoadIndexedPtr { base, index, .. } => {
                *base = resolve(&alias, *base);
                *index = resolve(&alias, *index);
            }
            IrInstruction::StoreIndexedPtr { base, index, src, .. } => {
                *base = resolve(&alias, *base);
                *index = resolve(&alias, *index);
                *src = resolve(&alias, *src);
            }
            IrInstruction::Call { args, .. } => {
                for a in args.iter_mut() {
                    *a = resolve(&alias, *a);
                }
            }
            IrInstruction::JumpIfZero { cond, .. } | IrInstruction::JumpIfNotZero { cond, .. } => {
                *cond = resolve(&alias, *cond);
            }
            IrInstruction::Return(Some(v)) => {
                *v = resolve(&alias, *v);
            }
            _ => {}
        }

        match func.instructions[i].clone() {
            IrInstruction::Label(_) => {
                // Cel skoku - inna ścieżka może mieć inny stan pamięci,
                // więc zapominamy o dotychczasowej wiedzy (bezpiecznie).
                last_store.clear();
            }
            IrInstruction::StoreMem { dst, src } => {
                last_store.insert(dst, (i, src, false));
            }
            IrInstruction::LoadMem { dst, src } => {
                if let Some((_, val, read)) = last_store.get_mut(&src) {
                    alias.insert(dst, *val);
                    *read = true;
                    to_remove.push(i);
                    changed = true;
                }
            }
            IrInstruction::LoadIndexed { .. } | IrInstruction::StoreIndexed { .. } => {
                // Indeks jest dynamiczny (nieznany w czasie kompilacji), więc
                // taki zapis/odczyt może w runtime trafić w DOWOLNY slot
                // tablicy. Bez tego zerowania forwarding mógłby podstawić
                // przeterminowaną (sprzed zapisu przez indeks) wartość dla
                // późniejszego odczytu o stałym indeksie tego samego slotu -
                // realny błąd poprawności, nie tylko utracona optymalizacja.
                last_store.clear();
            }
            _ => {}
        }
    }

    // Martwe zapisy: te, które nigdy nie zostały odczytane zanim funkcja
    // się skończyła lub zostały nadpisane (nadpisanie widać po tym, że
    // last_store dla danego slotu ma inny indeks niż ten zapis - śledzimy
    // to przez ponowne przejście).
    let mut overwritten_or_unread: Vec<usize> = Vec::new();
    {
        let mut current: HashMap<Location, (usize, bool)> = HashMap::new();
        for (i, instr) in func.instructions.iter().enumerate() {
            match instr {
                IrInstruction::Label(_) => current.clear(),
                IrInstruction::LoadMem { src, .. } => {
                    if let Some(entry) = current.get_mut(src) {
                        entry.1 = true;
                    }
                }
                IrInstruction::LoadIndexed { .. } | IrInstruction::StoreIndexed { .. } => {
                    // Tak samo jak wyżej: dynamiczny indeks mógł trafić w
                    // dowolny wcześniej zapisany slot tego samego arraya,
                    // więc żaden dotychczasowy zapis nie jest już bezpiecznie
                    // uznawany za "martwy" - traktujemy to jak granicę bloku.
                    current.clear();
                }
                IrInstruction::StoreMem { dst, .. } => {
                    if let Some((prev_idx, was_read)) = current.get(dst) {
                        if !was_read {
                            overwritten_or_unread.push(*prev_idx);
                        }
                    }
                    current.insert(dst.clone(), (i, false));
                }
                _ => {}
            }
        }
        for (_, (idx, was_read)) in current {
            if !was_read {
                overwritten_or_unread.push(idx);
            }
        }
    }

    for idx in overwritten_or_unread {
        if !matches!(func.instructions[idx], IrInstruction::Comment(_)) {
            to_remove.push(idx);
            changed = true;
        }
    }

    to_remove.sort_unstable();
    to_remove.dedup();
    for idx in to_remove.into_iter().rev() {
        func.instructions.remove(idx);
    }

    changed
}

/// Usuwa instrukcje czysto-funkcyjne, których wynik (dst) nigdy nie jest
/// używany dalej. `Call` nigdy nie jest usuwany (może mieć efekty uboczne),
/// ale jego `dst` jest czyszczony do `None`, jeśli wynik jest martwy.
fn dead_code_elim(func: &mut IrFunction) -> bool {
    let mut used: std::collections::HashSet<Temp> = std::collections::HashSet::new();
    for instr in &func.instructions {
        match instr {
            IrInstruction::BinaryOp { left, right, .. } => {
                used.insert(*left);
                used.insert(*right);
            }
            IrInstruction::StoreMem { src, .. } => {
                used.insert(*src);
            }
            IrInstruction::LoadIndexed { index, .. } => {
                used.insert(*index);
            }
            IrInstruction::StoreIndexed { index, src, .. } => {
                used.insert(*index);
                used.insert(*src);
            }
            IrInstruction::LoadIndexedPtr { base, index, .. } => {
                used.insert(*base);
                used.insert(*index);
            }
            IrInstruction::StoreIndexedPtr { base, index, src, .. } => {
                used.insert(*base);
                used.insert(*index);
                used.insert(*src);
            }
            IrInstruction::Call { args, .. } => {
                for a in args {
                    used.insert(*a);
                }
            }
            IrInstruction::JumpIfZero { cond, .. } | IrInstruction::JumpIfNotZero { cond, .. } => {
                used.insert(*cond);
            }
            IrInstruction::Return(Some(v)) => {
                used.insert(*v);
            }
            _ => {}
        }
    }

    let mut changed = false;
    let mut new_instrs = Vec::with_capacity(func.instructions.len());
    for instr in func.instructions.drain(..) {
        match instr {
            IrInstruction::LoadParam { dst, .. }
            | IrInstruction::LoadImm { dst, .. }
            | IrInstruction::LoadMem { dst, .. }
            | IrInstruction::LoadIndexed { dst, .. }
            | IrInstruction::LoadAddr { dst, .. }
            | IrInstruction::LoadIndexedPtr { dst, .. }
            | IrInstruction::BinaryOp { dst, .. }
                if !used.contains(&dst) =>
            {
                changed = true;
                // usunięte - brak efektów ubocznych (LoadIndexed/LoadAddr/
                // LoadIndexedPtr to czyste odczyty/obliczenia adresu,
                // bezpieczne do usunięcia tak samo jak LoadMem, gdy nikt nie
                // czyta wyniku)
                let _ = dst;
            }
            IrInstruction::Call { dst: Some(d), func: f, args } if !used.contains(&d) => {
                changed = true;
                new_instrs.push(IrInstruction::Call { dst: None, func: f, args });
            }
            other => new_instrs.push(other),
        }
    }
    func.instructions = new_instrs;
    changed
}

/// Peephole: usuwa `Jump L` gdy `L` jest zaraz następną instrukcją, oraz
/// kod nieosiągalny między bezwarunkowym skokiem/returnem a najbliższą etykietą.
fn peephole(func: &mut IrFunction) -> bool {
    let mut changed = false;

    // 1. Jump-to-next.
    let mut i = 0;
    while i + 1 < func.instructions.len() {
        if let IrInstruction::Jump(target) = &func.instructions[i] {
            if let IrInstruction::Label(l) = &func.instructions[i + 1] {
                if l == target {
                    func.instructions.remove(i);
                    changed = true;
                    continue;
                }
            }
        }
        i += 1;
    }

    // 2. Kod nieosiągalny po Jump/Return, aż do najbliższej etykiety.
    let mut new_instrs = Vec::with_capacity(func.instructions.len());
    let mut unreachable = false;
    for instr in func.instructions.drain(..) {
        match &instr {
            IrInstruction::Label(_) => {
                unreachable = false;
                new_instrs.push(instr);
            }
            _ if unreachable => {
                changed = true;
                // pomijamy - kod nieosiągalny
            }
            IrInstruction::Jump(_) | IrInstruction::Return(_) => {
                unreachable = true;
                new_instrs.push(instr);
            }
            _ => new_instrs.push(instr),
        }
    }
    func.instructions = new_instrs;

    // 3. Usuń komentarze zostawione po fold_branches (czysto kosmetyczne, ale
    //    tniemy rozmiar generowanego IR).
    func.instructions.retain(|i| !matches!(i, IrInstruction::Comment(_)));

    changed
}
