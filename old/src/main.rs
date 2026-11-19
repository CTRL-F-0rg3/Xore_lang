mod ast;
mod checker;
mod codegen_riscv64;
mod codegen_x86_64;
mod ir;
mod lexer;
mod link;
mod lowering;
mod module;
mod numlit;
mod optimize;
mod parser;
mod regalloc;
mod style;

use ast::Stmt;
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
    no_color: bool,
}

fn print_usage() {
    eprintln!("{}", style::banner("Użycie: xore_lang_new <plik.xre> [opcje]\n"));
    eprintln!("{}", style::banner("Opcje:"));
    eprintln!("  {}   architektura docelowa (domyślnie: x86_64)", style::code("--target=x86_64|riscv64"));
    eprintln!("  {}                 ścieżka pliku wyjściowego (domyślnie: nazwa źródła bez rozszerzenia)", style::code("-o <plik>"));
    eprintln!("  {}                  wyłącz optymalizacje IR", style::code("--no-opt"));
    eprintln!("  {}                 wypisz wygenerowane IR (po optymalizacjach) na stderr", style::code("--dump-ir"));
    eprintln!("  {}                zatrzymaj się na tekstowym asemblerze (.s), nie linkuj binarki", style::code("--emit-asm"));
    eprintln!("  {}                przy generacji binarki zachowaj też wygenerowany plik .s", style::code("--keep-asm"));
    eprintln!("  {}                 wyłącz kolorowanie komunikatów (ANSI)", style::code("--no-color"));
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
    let mut no_color = false;

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
        } else if arg == "--no-color" {
            no_color = true;
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
        no_color,
    })
}

fn run() -> Result<(), String> {
    let opts = parse_args().map_err(|e| {
        print_usage();
        style::fail(&e)
    })?;
    if opts.no_color {
        style::disable();
    }

    println!("{}", style::banner("=== Kompilator Xore ===\n"));

    // --- 1. Parsowanie + rozwinięcie `include` -------------------------------
    println!("{}", style::step("1. Parsowanie i rozwijanie `include`"));
    let mut module_system = ModuleSystem::new();
    let program = module_system.build_merged_program(&opts.input)?;
    println!("{}", style::ok(&format!("Sparsowano {} (wraz z include'ami), {} funkcji", opts.input, program.stmts.len())));

    // Zbierz flagi linkera: każde `ext 'c'` dokłada `-lc`, a dyrektywy
    // `link "..."` (z plików .xlib) dokładają flagi wprost.
    let mut link_flags: Vec<String> = Vec::new();
    for stmt in &program.stmts {
        match stmt {
            Stmt::ExternDecl { lang, .. } if lang == "c" => {
                if !link_flags.iter().any(|f| f == "-lc") {
                    link_flags.push("-lc".to_string());
                }
            }
            Stmt::Link { flags, .. } => {
                for f in flags {
                    if !link_flags.iter().any(|x| x == f) {
                        link_flags.push(f.clone());
                    }
                }
            }
            _ => {}
        }
    }

    // --- 2. Sprawdzanie semantyczne -------------------------------------------
    println!("{}", style::step("2. Sprawdzanie typów"));
    let mut checker = SemanticChecker::new();
    checker.check_program(&program);
    if checker.has_errors() {
        for e in checker.get_errors() {
            eprintln!("{}", style::fail(&format!("[{}..{}] {}", e.span.start, e.span.end, e.message)));
        }
        return Err(style::fail("Sprawdzanie typów nie powiodło się"));
    }
    println!("{}", style::ok("Sprawdzanie typów: brak błędów"));

    // --- 3. Lowering do IR ------------------------------------------------------
    println!("{}", style::step("3. Generowanie IR (lowering)"));
    let mut lowering = Lowering::new(opts.target);
    let mut ir_program = lowering
        .lower_program(&program)
        .map_err(|e| style::fail(&format!("Błąd generacji IR: {}", e.message)))?;
    println!("{}", style::ok(&format!("Wygenerowano IR ({} funkcji)", ir_program.functions.len())));

    // --- 4. Optymalizacje IR -----------------------------------------------------
    println!("{}", style::step("4. Optymalizacje IR"));
    if opts.optimize {
        optimize::optimize_program(&mut ir_program);
        println!("{}", style::ok("Zoptymalizowano IR"));
    } else {
        println!("{}", style::warn("Optymalizacje wyłączone (--no-opt)"));
    }

    if opts.dump_ir {
        eprintln!("\n{}\n{}", style::muted("--- IR ---"), ir_program);
    }

    // --- 5. Generacja kodu maszynowego (tekstowy asembler) -----------------------
    println!("{}", style::step("5. Generacja kodu maszynowego"));
    let asm = match opts.target {
        Target::X86_64 => codegen_x86_64::X86_64CodeGen::new().compile(&ir_program),
        Target::RiscV64 => codegen_riscv64::RiscV64CodeGen::new().compile(&ir_program),
    };

    let stem = Path::new(&opts.input).file_stem().and_then(|s| s.to_str()).unwrap_or("out");

    match opts.emit {
        Emit::Asm => {
            let out_path = opts.output.unwrap_or_else(|| format!("{}.{}.s", stem, opts.target.name()));
            fs::write(&out_path, asm).map_err(|e| style::fail(&format!("Nie można zapisać {}: {}", out_path, e)))?;
            println!("{}", style::ok(&format!("Zapisano kod maszynowy ({}) do {}", opts.target.name(), out_path)));
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
            fs::write(&asm_path, asm).map_err(|e| style::fail(&format!("Nie można zapisać {}: {}", asm_path.display(), e)))?;

            let link_result = link::assemble_and_link(opts.target, &asm_path, Path::new(&bin_out), &link_flags);

            if !opts.keep_asm {
                let _ = fs::remove_file(&asm_path);
            }

            link_result.map_err(|e| style::fail(&e))?;
            println!("{}", style::ok(&format!("Zbudowano binarkę ({}) -> {}", opts.target.name(), bin_out)));
            if opts.keep_asm {
                println!("{}", style::muted(&format!("(wygenerowany asembler zachowany w {})", asm_path.display())));
            }
        }
    }

    Ok(())
}

fn main() -> ExitCode {
    style::init_from_env();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::FAILURE
        }
    }
}
