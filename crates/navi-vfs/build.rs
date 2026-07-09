use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

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
    let cargo_home = cargo_home();

    // Ensure build-deps pull sources into the registry before we compile them.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_HOME");

    for g in GRAMMARS {
        let crate_dir = resolve_crate_dir(&cargo_home, g.name, g.version);
        let src_dir = crate_dir.join(g.subdir);
        let parser_c = src_dir.join("parser.c");
        if !parser_c.exists() {
            panic!(
                "Grammar source not found: {} (crate dir {})",
                parser_c.display(),
                crate_dir.display()
            );
        }

        let symbol = grammar_symbol(g);
        let so_path = out_dir.join(shared_lib_name(&symbol));

        // Skip if shared library already exists.
        if so_path.exists() {
            continue;
        }

        let cc = cc::Build::new().get_compiler();
        let is_msvc = cc.is_like_msvc();

        compile_c(&cc, &src_dir, &parser_c, &out_dir, &symbol, is_msvc);

        let mut objects = vec![obj_path(&out_dir, &symbol, "parser", is_msvc)];
        if g.has_scanner {
            let scanner_c = src_dir.join("scanner.c");
            if scanner_c.exists() {
                compile_c(&cc, &src_dir, &scanner_c, &out_dir, &symbol, is_msvc);
                objects.push(obj_path(&out_dir, &symbol, "scanner", is_msvc));
            }
        }

        link_shared(&cc, &so_path, &objects, is_msvc, g.name);

        println!("cargo:rerun-if-changed={}", parser_c.display());
    }
}

fn cargo_home() -> PathBuf {
    if let Ok(home) = env::var("CARGO_HOME") {
        return PathBuf::from(home);
    }
    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".cargo")
}

/// Locate `name-version` under registry/src, or extract it from registry/cache.
fn resolve_crate_dir(cargo_home: &Path, name: &str, version: &str) -> PathBuf {
    let want = format!("{name}-{version}");
    let src_root = cargo_home.join("registry").join("src");

    if src_root.is_dir() {
        if let Ok(entries) = fs::read_dir(&src_root) {
            for entry in entries.flatten() {
                let candidate = entry.path().join(&want);
                if candidate.is_dir() {
                    return candidate;
                }
            }
        }
    }

    // Extract from the downloaded .crate archive (always present once the
    // package is a build-dependency, even if `src/` was not unpacked yet).
    let cache_root = cargo_home.join("registry").join("cache");
    let crate_file = find_crate_file(&cache_root, &want)
        .unwrap_or_else(|| {
            panic!(
                "Could not find {want} in cargo registry src or cache under {}",
                cargo_home.display()
            )
        });

    // Prefer the first existing registry src index dir; create one if needed.
    let extract_root = if src_root.is_dir() {
        fs::read_dir(&src_root)
            .ok()
            .and_then(|mut it| it.find_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()))
            .unwrap_or_else(|| {
                let p = src_root.join("index.crates.io-extracted");
                fs::create_dir_all(&p).ok();
                p
            })
    } else {
        let p = src_root.join("index.crates.io-extracted");
        fs::create_dir_all(&p).expect("create registry src");
        p
    };

    let dest = extract_root.join(&want);
    if !dest.is_dir() {
        extract_crate(&crate_file, &extract_root, &want);
    }
    if !dest.is_dir() {
        panic!(
            "Failed to extract {} from {}",
            want,
            crate_file.display()
        );
    }
    dest
}

fn find_crate_file(cache_root: &Path, want: &str) -> Option<PathBuf> {
    if !cache_root.is_dir() {
        return None;
    }
    let entries = fs::read_dir(cache_root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path().join(format!("{want}.crate"));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn extract_crate(crate_file: &Path, extract_root: &Path, want: &str) {
    fs::create_dir_all(extract_root).ok();
    // .crate files are gzip-compressed tar archives.
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(crate_file)
        .arg("-C")
        .arg(extract_root)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => panic!("tar extract of {} failed: {s}", crate_file.display()),
        Err(e) => {
            // Windows runners may lack tar in PATH for some shells; try bsdtar.
            let status2 = Command::new("bsdtar")
                .arg("-xzf")
                .arg(crate_file)
                .arg("-C")
                .arg(extract_root)
                .status();
            match status2 {
                Ok(s) if s.success() => {}
                _ => panic!(
                    "tar extract of {} failed ({e}); expected directory {want}",
                    crate_file.display()
                ),
            }
        }
    }
}

fn obj_path(out_dir: &Path, symbol: &str, stem: &str, is_msvc: bool) -> PathBuf {
    let ext = if is_msvc { "obj" } else { "o" };
    out_dir.join(format!("{symbol}_{stem}.{ext}"))
}

fn compile_c(
    cc: &cc::Tool,
    src_dir: &Path,
    src: &Path,
    out_dir: &Path,
    symbol: &str,
    is_msvc: bool,
) {
    let stem = src.file_stem().unwrap_or_default().to_str().unwrap_or("x");
    let out_obj = obj_path(out_dir, symbol, stem, is_msvc);
    let mut cmd = cc.to_command();
    if is_msvc {
        // cl.exe /nologo /std:c11 /c /Foout.obj /Iinclude file.c
        cmd.arg("/nologo")
            .arg("/std:c11")
            .arg("/c")
            .arg(format!("/Fo{}", out_obj.display()))
            .arg(format!("/I{}", src_dir.display()))
            .arg(src);
    } else {
        cmd.arg("-std=c11")
            .arg("-fPIC")
            .arg("-I")
            .arg(src_dir)
            .arg("-c")
            .arg("-o")
            .arg(&out_obj)
            .arg(src);
    }
    run(&mut cmd, &format!("cc -c {} ({})", stem, symbol));
}

fn link_shared(cc: &cc::Tool, so_path: &Path, objects: &[PathBuf], is_msvc: bool, name: &str) {
    let mut cmd = cc.to_command();
    if is_msvc {
        // cl.exe /nologo /LD /Fe:out.dll a.obj b.obj
        cmd.arg("/nologo")
            .arg("/LD")
            .arg(format!("/Fe:{}", so_path.display()));
        for obj in objects {
            cmd.arg(obj);
        }
    } else {
        cmd.arg("-shared").arg("-o").arg(so_path).arg("-lc");
        for obj in objects {
            cmd.arg(obj);
        }
    }
    run(&mut cmd, &format!("link {name}"));
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

fn run(cmd: &mut Command, label: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("{label} failed: {e}"));
    assert!(status.success(), "{label} exited with {status}");
}
