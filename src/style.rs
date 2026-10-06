//! Kolorystyka i formatowanie komunikatów kompilatora (motyw magentowy).
//!
//! Akcenty i nagłówki są magentowe, komunikaty sukcesu pogrubione zielone,
//! błędy pogrubione czerwone, ostrzeżenia pogrubione żółte, a detale cyjanowe.
//!
//! Kolory są automatycznie wyłączane, gdy wyjście nie jest terminalem, gdy
//! ustawiono zmienną `NO_COLOR` lub gdy podano flagę `--no-color`. Zmienna
//! `FORCE_COLOR` pozwala wymusić kolory (np. przy przekierowaniu do pliku).

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(true);

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const BRIGHT_MAGENTA: &str = "\x1b[95m";
const BRIGHT_GREEN: &str = "\x1b[92m";
const BRIGHT_RED: &str = "\x1b[91m";
const BRIGHT_YELLOW: &str = "\x1b[93m";
const BRIGHT_CYAN: &str = "\x1b[96m";

fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Ustala włączanie kolorów na podstawie środowiska (NO_COLOR, FORCE_COLOR, TTY).
pub fn init_from_env() {
    let no_color = std::env::var_os("NO_COLOR").map(|v| !v.is_empty()).unwrap_or(false);
    let force = std::env::var_os("FORCE_COLOR").map(|v| !v.is_empty()).unwrap_or(false);
    let tty = std::io::stdout().is_terminal() || std::io::stderr().is_terminal();

    let on = if no_color { false } else { force || tty };
    ENABLED.store(on, Ordering::Relaxed);
}

/// Wymusza wyłączenie kolorów (flaga `--no-color`).
pub fn disable() {
    ENABLED.store(false, Ordering::Relaxed);
}

fn paint(text: &str, codes: &[&str]) -> String {
    if !enabled() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 24);
    for code in codes {
        out.push_str(code);
    }
    out.push_str(text);
    out.push_str(RESET);
    out
}

/// Nagłówek kompilatora / sekcji — pogrubiony magentowy.
pub fn banner(text: &str) -> String {
    paint(text, &[BOLD, BRIGHT_MAGENTA])
}

/// Nagłówek etapu kompilacji.
pub fn step(text: &str) -> String {
    paint(&format!("── {} ──", text), &[BOLD, BRIGHT_MAGENTA])
}

/// Komunikat sukcesu — pogrubiony zielony z prefiksem ✔.
pub fn ok(text: &str) -> String {
    paint(&format!("✔ {}", text), &[BOLD, BRIGHT_GREEN])
}

/// Komunikat błędu — pogrubiony czerwony z prefiksem ✘.
pub fn fail(text: &str) -> String {
    paint(&format!("✘ {}", text), &[BOLD, BRIGHT_RED])
}

/// Komunikat ostrzeżenia — pogrubiony żółty z prefiksem ⚠.
pub fn warn(text: &str) -> String {
    paint(&format!("⚠ {}", text), &[BOLD, BRIGHT_YELLOW])
}

/// Nazwa opcji / fragment kodu w treści komunikatu — cyjan.
pub fn code(text: &str) -> String {
    paint(text, &[BRIGHT_CYAN])
}

/// Przygaszony tekst pomocniczy (np. dodatkowe ścieżki).
pub fn muted(text: &str) -> String {
    paint(text, &[DIM])
}

// --- Diagnostyka (fragment źródła z podkreśleniem błędu) ---------------------

/// Oblicza (linia, kolumna) — obie 1-indeksowane — dla bajtowego offsetu.
fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let before = &source[..offset];
    let line = before.matches('\n').count() + 1;
    let col = offset - before.rfind('\n').map(|i| i + 1).unwrap_or(0) + 1;
    (line, col)
}

/// Szerokość wyświetlana (taby liczone jako 4 spacje).
fn display_width(s: &str) -> usize {
    s.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

/// Spacje wyrównujące podkładkę pod znakiem `^` (taby -> 4 spacje).
fn caret_pad(s: &str) -> String {
    s.chars().map(|c| if c == '\t' { "    " } else { " " }).collect()
}

/// Renderuje błąd w stylu rustc: nagłówek, lokalizacja `plik:linia:kolumna`,
/// linia źródła oraz podkreślenie `^^^`. Zwraca gotowy, pokolorowany tekst.
pub fn render_error(source: &str, path: &str, start: usize, end: usize, message: &str) -> String {
    let start = start.min(source.len());
    let end = end.max(start).min(source.len());

    let (line_no, col) = line_col(source, start);

    let line_start = source[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = source[line_start..].find('\n').map(|i| line_start + i).unwrap_or(source.len());
    let line_text = source[line_start..line_end].trim_end_matches('\r');
    let line_display = line_text.replace('\t', "    ");

    let span_end = end.min(line_end);
    let span_text = &source[start..span_end];
    let caret_len = display_width(span_text).max(1);
    let pad = caret_pad(&source[line_start..start]);

    let gutter = line_no.to_string();
    let blank = " ".repeat(gutter.len());

    let mut out = String::new();
    out.push_str(&format!(
        "{} {}\n",
        paint("błąd:", &[BOLD, BRIGHT_RED]),
        message
    ));
    out.push_str(&format!(
        "{} {}:{}:{}\n",
        paint("-->", &[BRIGHT_MAGENTA]),
        path,
        line_no,
        col
    ));
    out.push_str(&format!("{} |\n", blank));
    out.push_str(&format!(
        "{} | {}\n",
        paint(&gutter, &[BOLD, BRIGHT_MAGENTA]),
        line_display
    ));
    out.push_str(&format!(
        "{} | {}{}\n",
        blank,
        pad,
        paint(&"^".repeat(caret_len), &[BOLD, BRIGHT_RED])
    ));
    out
}
