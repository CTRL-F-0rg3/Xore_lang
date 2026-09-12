mod ast;
mod checker;
mod codegen_riscv64;
mod codegen_x86_64;
mod ir;
mod lexer;
mod link;
mod lowering;
mod module;
mod optimize;
mod parser;
mod regalloc;

use checker::SemanticChecker;
use lowering::Lowering;
use module::ModuleSystem;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    X86_64,
    RiscV64,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::X86_64 => "x86_64",
            Target::RiscV64 => "riscv64",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emit {
    /// Domyślnie: kompilator sam wywołuje asembler/linker i oddaje gotową
    /// binarkę - etap generacji `.s` jest wewnętrzny i niewidoczny.
    Binary,
    /// Zatrzymaj się na etapie tekstowego asemblera (dla ciekawych/debugowania).
    Asm,
}

struct Options {
    input: String,
    target: Target,
    output: Option<String>,
    optimize: bool,
    dump_ir: bool,
    emit: Emit,
    keep_asm: bool,
}

fn print_usage() {
    eprintln!("Użycie: xore_lang_new <plik.xre> [opcje]\n");
    eprintln!("Opcje:");
    eprintln!("  --target=x86_64|riscv64   architektura docelowa (domyślnie: x86_64)");
    eprintln!("  -o <plik>                 ścieżka pliku wyjściowego (domyślnie: nazwa źródła bez rozszerzenia)");
    eprintln!("  --no-opt                  wyłącz optymalizacje IR");
    eprintln!("  --dump-ir                 wypisz wygenerowane IR (po optymalizacjach) na stderr");
    eprintln!("  --emit-asm                zatrzymaj się na tekstowym asemblerze (.s), nie linkuj binarki");
    eprintln!("  --keep-asm                przy generacji binarki zachowaj też wygenerowany plik .s");
}

fn parse_args() -> Result<Options, String> {
    let mut args = env::args().skip(1);
    let mut input = None;
    let mut target = Target::X86_64;
    let mut output = None;
    let mut optimize = true;
    let mut dump_ir = false;
    let mut emit = Emit::Binary;
    let mut keep_asm = false;

    while let Some(arg) = args.next() {
        if let Some(t) = arg.strip_prefix("--target=") {
            target = match t {
                "x86_64" => Target::X86_64,
                "riscv64" => Target::RiscV64,
                other => return Err(format!("Nieznana architektura docelowa: {}", other)),
            };
        } else if arg == "-o" {
            output = Some(args.next().ok_or("Brak argumentu dla -o")?);
        } else if arg == "--no-opt" {
            optimize = false;
        } else if arg == "--dump-ir" {
            dump_ir = true;
        } else if arg == "--emit-asm" {
            emit = Emit::Asm;
        } else if arg == "--keep-asm" {
            keep_asm = true;
        } else if arg == "-h" || arg == "--help" {
            print_usage();
            std::process::exit(0);
        } else if input.is_none() {
            input = Some(arg);
        } else {
            return Err(format!("Nieoczekiwany argument: {}", arg));
        }
    }

    Ok(Options {
        input: input.ok_or("Nie podano pliku wejściowego")?,
        target,
        output,
        optimize,
        dump_ir,
        emit,
        keep_asm,
    })
}

fn run() -> Result<(), String> {
    let opts = parse_args().map_err(|e| {
        print_usage();
        e
    })?;

    println!("=== Kompilator Xore ===\n");

    // --- 1. Parsowanie + rozwinięcie `include` -------------------------------
    let mut module_system = ModuleSystem::new();
    let program = module_system
        .build_merged_program(&opts.input)
        .map_err(|e| format!("❌ Błąd wczytywania/parsowania: {}", e))?;
    println!("✅ Sparsowano {} (wraz z include'ami), {} funkcji", opts.input, program.stmts.len());

    // --- 2. Sprawdzanie semantyczne -------------------------------------------
    let mut checker = SemanticChecker::new();
    checker.check_program(&program);
    if checker.has_errors() {
        for e in checker.get_errors() {
            eprintln!("❌ [{}..{}] {}", e.span.start, e.span.end, e.message);
        }
        return Err("Sprawdzanie typów nie powiodło się".to_string());
    }
    println!("✅ Sprawdzanie typów: brak błędów");

    // --- 3. Lowering do IR ------------------------------------------------------
    let mut lowering = Lowering::new();
    let mut ir_program = lowering
        .lower_program(&program)
        .map_err(|e| format!("❌ Błąd generacji IR: {}", e.message))?;
    println!("✅ Wygenerowano IR ({} funkcji)", ir_program.functions.len());

    // --- 4. Optymalizacje IR -----------------------------------------------------
    if opts.optimize {
        optimize::optimize_program(&mut ir_program);
        println!("✅ Zoptymalizowano IR");
    } else {
        println!("⚠️  Optymalizacje wyłączone (--no-opt)");
    }

    if opts.dump_ir {
        eprintln!("\n--- IR ---\n{}", ir_program);
    }

    // --- 5. Generacja kodu maszynowego (tekstowy asembler) -----------------------
    let asm = match opts.target {
        Target::X86_64 => codegen_x86_64::X86_64CodeGen::new().compile(&ir_program),
        Target::RiscV64 => codegen_riscv64::RiscV64CodeGen::new().compile(&ir_program),
    };

    let stem = Path::new(&opts.input).file_stem().and_then(|s| s.to_str()).unwrap_or("out");

    match opts.emit {
        Emit::Asm => {
            let out_path = opts.output.unwrap_or_else(|| format!("{}.{}.s", stem, opts.target.name()));
            fs::write(&out_path, asm).map_err(|e| format!("Nie można zapisać {}: {}", out_path, e))?;
            println!("✅ Zapisano kod maszynowy ({}) do {}", opts.target.name(), out_path);
        }
        Emit::Binary => {
            // Etap tekstowego asemblera jest tu wewnętrzny: piszemy go do
            // pliku tymczasowego, wywołujemy systemowy asembler/linker i
            // (domyślnie) sprzątamy po sobie - użytkownik dostaje od razu
            // gotową binarkę, bez ręcznego kroku pośredniego.
            let bin_out = opts.output.unwrap_or_else(|| stem.to_string());
            let asm_path: PathBuf = if opts.keep_asm {
                PathBuf::from(format!("{}.{}.s", stem, opts.target.name()))
            } else {
                std::env::temp_dir().join(format!("xore_{}_{}.s", stem, std::process::id()))
            };
            fs::write(&asm_path, asm).map_err(|e| format!("Nie można zapisać {}: {}", asm_path.display(), e))?;

            let link_result = link::assemble_and_link(opts.target, &asm_path, Path::new(&bin_out));

            if !opts.keep_asm {
                let _ = fs::remove_file(&asm_path);
            }

            link_result?;
            println!("✅ Zbudowano binarkę ({}) -> {}", opts.target.name(), bin_out);
            if opts.keep_asm {
                println!("   (wygenerowany asembler zachowany w {})", asm_path.display());
            }
        }
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::FAILURE
        }
    }
}
