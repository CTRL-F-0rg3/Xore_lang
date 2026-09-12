use crate::ast::{Program, Stmt, Visibility};
use crate::parser::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;

#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub name: String,
    pub params: Vec<(String, crate::ast::Type)>,
    pub return_type: Option<crate::ast::Type>,
    pub visibility: Visibility,
}

#[derive(Debug)]
pub struct Module {
    pub name: String,
    pub path: PathBuf,
    pub program: Program,
    pub declarations: HashMap<String, FunctionSignature>,
    pub definitions: HashMap<String, FunctionSignature>,
}

pub struct ModuleSystem {
    modules: HashMap<String, Module>,
    include_paths: Vec<PathBuf>,
}

impl ModuleSystem {
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            include_paths: vec![PathBuf::from(".")],
        }
    }

    pub fn add_include_path(&mut self, path: PathBuf) {
        self.include_paths.push(path);
    }

    pub fn load_file(&mut self, path: &str) -> Result<String, String> {
        let path_buf = PathBuf::from(path);

        let ext = path_buf.extension().and_then(|e| e.to_str()).ok_or("No file extension")?;
        if ext != "xre" && ext != "xrh" {
            return Err(format!("Unsupported file extension: {}", ext));
        }

        let source = fs::read_to_string(&path_buf)
        .map_err(|e| format!("Cannot read file {}: {}", path, e))?;

        let mut parser = Parser::new(&source);
        let program = parser.parse_program();

        if parser.has_errors() {
            let details = parser.get_errors().iter()
                .map(|e| format!("{:?} @ {}..{}", e.message, e.span.start, e.span.end))
                .collect::<Vec<_>>().join("; ");
            return Err(format!("Parse errors in {}: {}", path, details));
        }

        let mut declarations = HashMap::new();
        let mut definitions = HashMap::new();

        for stmt in &program.stmts {
            match stmt {
                Stmt::FnDecl { name, params, return_type, visibility, .. } => {
                    declarations.insert(name.clone(), FunctionSignature {
                        name: name.clone(),
                                        params: params.clone(),
                                        return_type: return_type.clone(),
                                        visibility: visibility.clone(),
                    });
                }
                Stmt::FnDef { name, params, return_type, visibility, .. } => {
                    definitions.insert(name.clone(), FunctionSignature {
                        name: name.clone(),
                                       params: params.clone(),
                                       return_type: return_type.clone(),
                                       visibility: visibility.clone(),
                    });
                }
                _ => {}
            }
        }

        let module_name = path_buf.file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Invalid file name")?
        .to_string();

        self.modules.insert(module_name.clone(), Module {
            name: module_name,
            path: path_buf,
            program,
            declarations,
            definitions,
        });

        Ok(source)
    }

    /// Ładuje plik wejściowy i rekurencyjnie rozwija jego `include "...";`,
    /// scalając wszystkie znalezione definicje funkcji (`FnDef`) w jeden
    /// `Program` gotowy do sprawdzenia typów i generacji kodu.
    ///
    /// Deklaracje (`FnDecl`, np. z plików `.xrh`) same w sobie nie trafiają
    /// do wynikowego programu - służą tylko do tego, by `include` nie
    /// zgłaszał braku pliku; realna implementacja funkcji musi znaleźć się
    /// w którymś z załadowanych plików (dokładnie tak jak nagłówek `.h` w C
    /// nie generuje kodu sam z siebie).
    pub fn build_merged_program(&mut self, entry_path: &str) -> Result<Program, String> {
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut defs: HashMap<String, Stmt> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        self.collect_defs(entry_path, &mut visited, &mut defs, &mut order)?;

        let stmts = order.into_iter().filter_map(|name| defs.remove(&name)).collect();
        Ok(Program { stmts })
    }

    fn collect_defs(
        &mut self,
        path: &str,
        visited: &mut std::collections::HashSet<String>,
        defs: &mut HashMap<String, Stmt>,
        order: &mut Vec<String>,
    ) -> Result<(), String> {
        let canonical = path.to_string();
        if visited.contains(&canonical) {
            return Ok(());
        }
        visited.insert(canonical);

        self.load_file(path)?;
        let module_name = PathBuf::from(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("Invalid file name")?
            .to_string();

        // Sklonujmy statementy, żeby nie trzymać pożyczenia z self.modules
        // podczas rekurencyjnego wywołania load_file dla include'ów.
        let stmts = self.modules.get(&module_name).unwrap().program.stmts.clone();
        let dir = PathBuf::from(path).parent().map(|p| p.to_path_buf()).unwrap_or_default();

        for stmt in stmts {
            match &stmt {
                Stmt::Include { path: inc, .. } => {
                    let inc_path = dir.join(inc);
                    let inc_str = inc_path.to_string_lossy().to_string();
                    self.collect_defs(&inc_str, visited, defs, order)?;
                }
                Stmt::FnDef { name, .. } => {
                    if !defs.contains_key(name) {
                        order.push(name.clone());
                    }
                    // Definicja w pliku ładowanym później nadpisuje wcześniejszą
                    // deklarację/definicję o tej samej nazwie (jak w C: .c > .h).
                    defs.insert(name.clone(), stmt);
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn find_function(&self, name: &str) -> Option<&FunctionSignature> {
        for module in self.modules.values() {
            if let Some(sig) = module.definitions.get(name) {
                return Some(sig);
            }
            if let Some(sig) = module.declarations.get(name) {
                return Some(sig);
            }
        }
        None
    }
}
