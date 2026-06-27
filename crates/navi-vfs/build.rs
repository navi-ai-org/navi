use std::{env, path::PathBuf};

struct Grammar {
    name: &'static str,
    version: &'static str,
    subdir: &'static str,
    has_scanner: bool,
}

const GRAMMARS: &[Grammar] = &[
    Grammar {
        name: "tree-sitter-rust",
        version: "0.24.2",
        subdir: "src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-python",
        version: "0.25.0",
        subdir: "src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-javascript",
        version: "0.25.0",
        subdir: "src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-typescript",
        version: "0.23.2",
        subdir: "typescript/src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-typescript",
        version: "0.23.2",
        subdir: "tsx/src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-go",
        version: "0.25.0",
        subdir: "src",
        has_scanner: false,
    },
    Grammar {
        name: "tree-sitter-c",
        version: "0.24.2",
        subdir: "src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-cpp",
        version: "0.23.4",
        subdir: "src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-java",
        version: "0.23.5",
        subdir: "src",
        has_scanner: false,
    },
    Grammar {
        name: "tree-sitter-ruby",
        version: "0.23.1",
        subdir: "src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-php",
        version: "0.24.2",
        subdir: "php/src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-bash",
        version: "0.25.1",
        subdir: "src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-html",
        version: "0.23.2",
        subdir: "src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-css",
        version: "0.25.0",
        subdir: "src",
        has_scanner: true,
    },
    Grammar {
        name: "tree-sitter-json",
        version: "0.24.8",
        subdir: "src",
        has_scanner: false,
    },
    Grammar {
        name: "tree-sitter-c-sharp",
        version: "0.23.5",
        subdir: "src",
        has_scanner: true,
    },
];

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let registry = find_registry_src();

    for g in GRAMMARS {
        let crate_dir = registry.join(format!("{}-{}", g.name, g.version));
        let src_dir = crate_dir.join(g.subdir);
        let parser_c = src_dir.join("parser.c");
        if !parser_c.exists() {
            panic!(
                "Grammar source not found: {} (expected in {})",
                parser_c.display(),
                crate_dir.display()
            );
        }

        let symbol = grammar_symbol(g);
        let so_path = out_dir.join(shared_lib_name(&symbol));

        // Skip if .so already exists.
        if so_path.exists() {
            continue;
        }

        let cc = cc::Build::new().get_compiler();

        // Compile parser.c → {symbol}_parser.o
        compile_c(&cc, &src_dir, &parser_c, &out_dir, &symbol);

        // Compile scanner.c → {symbol}_scanner.o (if present)
        if g.has_scanner {
            let scanner_c = src_dir.join("scanner.c");
            if scanner_c.exists() {
                compile_c(&cc, &src_dir, &scanner_c, &out_dir, &symbol);
            }
        }

        // Link .o files → .so
        let mut link = cc.to_command();
        link.arg("-shared").arg("-o").arg(&so_path).arg("-lc");
        link_obj(&mut link, &out_dir, &symbol, "parser");
        if g.has_scanner {
            let scanner_c = src_dir.join("scanner.c");
            if scanner_c.exists() {
                link_obj(&mut link, &out_dir, &symbol, "scanner");
            }
        }
        run(&mut link, &format!("link {}", g.name));

        println!("cargo:rerun-if-changed={}", parser_c.display());
    }
}

fn compile_c(
    cc: &cc::Tool,
    src_dir: &std::path::Path,
    src: &std::path::Path,
    out_dir: &PathBuf,
    symbol: &str,
) {
    let stem = src.file_stem().unwrap_or_default().to_str().unwrap_or("x");
    let out_obj = out_dir.join(format!("{symbol}_{stem}.o"));
    let mut cmd = cc.to_command();
    cmd.arg("-std=c11")
        .arg("-fPIC")
        .arg("-I")
        .arg(src_dir)
        .arg("-c")
        .arg("-o")
        .arg(&out_obj)
        .arg(src);
    run(&mut cmd, &format!("cc -c {} ({})", stem, symbol));
}

fn link_obj(cmd: &mut std::process::Command, out_dir: &PathBuf, symbol: &str, stem: &str) {
    cmd.arg(out_dir.join(format!("{symbol}_{stem}.o")));
}

fn find_registry_src() -> PathBuf {
    let cargo_home = env::var("CARGO_HOME")
        .unwrap_or_else(|_| format!("{}/.cargo", env::var("HOME").unwrap_or_default()));
    let src = PathBuf::from(cargo_home).join("registry").join("src");
    for entry in std::fs::read_dir(&src).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            return entry.path();
        }
    }
    panic!("No cargo registry src in {}", src.display());
}

fn grammar_symbol(g: &Grammar) -> String {
    let prefix = g.name.strip_prefix("tree-sitter-").unwrap_or(g.name);
    let s = if g.subdir == "src" {
        format!("tree_sitter_{prefix}")
    } else {
        let dir_name = g.subdir.split('/').next().unwrap_or(prefix);
        format!("tree_sitter_{dir_name}")
    };
    s.replace('-', "_")
}

fn shared_lib_name(symbol: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("lib{}.dylib", symbol)
    } else if cfg!(target_os = "windows") {
        format!("{}.dll", symbol)
    } else {
        format!("lib{}.so", symbol)
    }
}

fn run(cmd: &mut std::process::Command, label: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("{label} failed: {e}"));
    assert!(status.success(), "{label} exited with {status}");
}
