//! Ukrywa etap "asemblacja + linkowanie" przed użytkownikiem: kompilator
//! sam wywołuje systemowy asembler/linker (poprzez `cc`/`gcc` jako driver -
//! to standardowa praktyka, tak samo robi `rustc` czy `clang`) i oddaje
//! gotowy plik wykonywalny. Sam generator kodu (`codegen_x86_64.rs`,
//! `codegen_riscv64.rs`) nadal produkuje wyłącznie tekstowy asembler - nic
//! się tu nie zmienia w sposobie generacji kodu, zmienia się tylko to, że
//! ten etap przestaje być widoczny/ręczny dla użytkownika.

use std::path::Path;
use std::process::Command;

use crate::Target;

/// Lista kandydatów na driver asemblera/linkera dla danej architektury,
/// sprawdzana po kolei - pierwszy znaleziony w PATH zostaje użyty.
fn candidate_drivers(target: Target) -> &'static [&'static str] {
    match target {
        Target::X86_64 => &["cc", "gcc", "clang"],
        Target::RiscV64 => &[
            "riscv64-linux-gnu-gcc",
            "riscv64-unknown-linux-gnu-gcc",
            "riscv64-elf-gcc",
            "riscv64-linux-gnu-gcc-13",
        ],
    }
}

fn find_driver(target: Target) -> Option<String> {
    for candidate in candidate_drivers(target) {
        let found = Command::new("which")
            .arg(candidate)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if found {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Asembluje i linkuje `asm_path` do gotowej binarki `out_path`.
///
/// Zwraca `Ok(())` gdy binarka powstała. Gdy odpowiedni toolchain (np.
/// cross-kompilator RISC-V) nie jest zainstalowany na tej maszynie, zwraca
/// czytelny błąd zamiast udawać sukces - plik `.s` zostaje na dysku, więc
/// nic nie ginie.
pub fn assemble_and_link(target: Target, asm_path: &Path, out_path: &Path) -> Result<(), String> {
    let driver = find_driver(target).ok_or_else(|| {
        let candidates = candidate_drivers(target).join(", ");
        format!(
            "Nie znaleziono asemblera/linkera dla {} (szukano: {}).\n\
             Wygenerowany asembler zostawiłem w {} - możesz go zasemblować ręcznie\n\
             po zainstalowaniu odpowiedniego toolchaina, np.:\n  \
             apt-get install {}",
            target.name(),
            candidates,
            asm_path.display(),
            match target {
                Target::X86_64 => "gcc".to_string(),
                Target::RiscV64 => "gcc-riscv64-linux-gnu binutils-riscv64-linux-gnu".to_string(),
            }
        )
    })?;

    let mut cmd = Command::new(&driver);
    cmd.arg(asm_path)
        .arg("-o")
        .arg(out_path)
        .arg("-static")
        // Xore jest zawsze budowany jako plik freestanding: bez libc, bez
        // crt0/crt1 - punkt wejścia to nasz własny `_start` (patrz
        // codegen_x86_64.rs / codegen_riscv64.rs). `cc`/`gcc` tu pełnią
        // wyłącznie rolę drivera do systemowego asemblera/linkera - nie
        // wnoszą żadnego kodu ani zależności runtime'owych.
        .arg("-nostdlib")
        .arg("-e")
        .arg("_start")
        // Tłumi nieszkodliwe ostrzeżenie linkera o brakującej sekcji
        // .note.GNU-stack (nasz asembler jej nie emituje, bo i tak nie
        // generujemy kodu na stosie).
        .arg("-Wa,--noexecstack");
    if target == Target::X86_64 {
        cmd.arg("-no-pie");
    }

    let output = cmd
        .output()
        .map_err(|e| format!("Nie udało się uruchomić '{}': {}", driver, e))?;

    if !output.status.success() {
        return Err(format!(
            "Asemblacja/linkowanie ({}) nie powiodło się:\n{}",
            driver,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}
