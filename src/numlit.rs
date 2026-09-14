//! Parsowanie literałów całkowitych Xore - dzielone między `checker.rs`
//! (do wnioskowania typu) i `lowering.rs` (do wygenerowania właściwej
//! wartości `LoadImm`). Wydzielone do jednego miejsca, żeby oba etapy
//! zawsze zgadzały się co do tego, jak dany literał ma być zinterpretowany
//! - rozjazd między nimi był źródłem poprzedniego buga (sufiks `_i64`
//! poprawnie rozpoznawany przez jeden, ale nierozumiany przez drugi).

use crate::ast::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntSuffix {
    I32,
    I64,
    U32,
    U64,
}

#[derive(Debug, Clone, Copy)]
pub struct ParsedInt {
    /// Surowe 64 bity wartości. Dla literałów unsigned, których wartość nie
    /// mieści się w `i64`, to jest bitowa reinterpretacja `u64` - poprawna
    /// do dalszego przetwarzania, o ile operacje na niej (porównania,
    /// dzielenie) też traktują ją jako unsigned (patrz `IrBinOp::*U`).
    pub bits: i64,
    /// Jawny sufiks typu (`_i32`, `_i64`, `_u32`, `_u64`), jeśli podany.
    pub suffix: Option<IntSuffix>,
}

/// Parsuje tekst literału (tak jak wychodzi z lexera - z ewentualnym
/// prefiksem `0x`/`0b`/`0o`, separatorami `_` między cyframi i opcjonalnym
/// sufiksem typu na końcu) na wartość i sufiks.
pub fn parse_int_literal(text: &str) -> ParsedInt {
    let (num_part, suffix) = split_suffix(text);
    let bits = parse_value(num_part);
    ParsedInt { bits, suffix }
}

fn split_suffix(text: &str) -> (&str, Option<IntSuffix>) {
    const SUFFIXES: [(&str, IntSuffix); 4] = [
        ("_i32", IntSuffix::I32),
        ("_i64", IntSuffix::I64),
        ("_u32", IntSuffix::U32),
        ("_u64", IntSuffix::U64),
    ];
    for (s, suf) in SUFFIXES {
        if let Some(stripped) = text.strip_suffix(s) {
            return (stripped, Some(suf));
        }
    }
    (text, None)
}

fn parse_value(s: &str) -> i64 {
    let clean: String = s.chars().filter(|&c| c != '_').collect();
    let (radix, digits): (u32, &str) =
        if let Some(r) = clean.strip_prefix("0x").or_else(|| clean.strip_prefix("0X")) {
            (16, r)
        } else if let Some(r) = clean.strip_prefix("0b").or_else(|| clean.strip_prefix("0B")) {
            (2, r)
        } else if let Some(r) = clean.strip_prefix("0o").or_else(|| clean.strip_prefix("0O")) {
            (8, r)
        } else {
            (10, clean.as_str())
        };

    // Najpierw próbujemy jako i64 (typowy przypadek). Jeśli wartość jest za
    // duża (np. literał bliski u64::MAX z sufiksem `_u64`), próbujemy jako
    // u64 i bitowo reinterpretujemy - to daje poprawny bitowy wzorzec do
    // dalszych operacji unsigned, mimo że "jako i64" byłby ujemny.
    if let Ok(v) = i64::from_str_radix(digits, radix) {
        v
    } else if let Ok(v) = u64::from_str_radix(digits, radix) {
        v as i64
    } else {
        0
    }
}

/// Wnioskuje typ Xore dla literału całkowitego: jawny sufiks wygrywa;
/// bez sufiksu domyślnie `i32`, chyba że wartość się w nim nie mieści -
/// wtedy `i64`. To była realna dziura: literał bez sufiksu zawsze dostawał
/// `Type::I32` niezależnie od wartości, więc np. `let x: i64 = 9999999999999;`
/// nigdy nie przechodziło type-checkingu, mimo że wartość jest poprawna dla i64.
pub fn infer_int_type(parsed: &ParsedInt) -> Type {
    match parsed.suffix {
        Some(IntSuffix::I32) => Type::I32,
        Some(IntSuffix::I64) => Type::I64,
        Some(IntSuffix::U32) => Type::U32,
        Some(IntSuffix::U64) => Type::U64,
        None => {
            if parsed.bits >= i32::MIN as i64 && parsed.bits <= i32::MAX as i64 {
                Type::I32
            } else {
                Type::I64
            }
        }
    }
}

pub fn is_unsigned(ty: &Type) -> bool {
    matches!(ty, Type::U32 | Type::U64)
}
