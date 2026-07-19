//! Runtime dynamic loading of tree-sitter grammar shared libraries.
//!
//! Grammars are compiled to `.so`/`.dylib` files by [`build.rs`](build.rs)
//! and loaded lazily on first use.  The loaded `Library` must outlive the
//! returned `Language` (which contains a raw pointer to the `.so`'s data).

use std::{
    path::PathBuf,
    sync::{LazyLock, Mutex},
};

use anyhow::{Context, Result};
use libloading::Library;
use tree_sitter::Language;
use tree_sitter_language::LanguageFn;

use crate::lang::LangId;

/// Cached grammar libraries.
static CACHE: LazyLock<Mutex<Vec<(LangId, LoadedGrammar)>>> =
    LazyLock::new(|| Mutex::new(Vec::with_capacity(16)));

struct LoadedGrammar {
    /// Keep the library alive; dropping it would unmap the Language's data.
    /// Prefixed with `_` because it is only held for its Drop side effect.
    _library: Library,
    language: Language,
}

/// Return the tree-sitter `Language` for `lang`, loading the corresponding
/// `.so` on first access.
pub(crate) fn load_language(lang: LangId) -> Result<Language> {
    let mut cache = CACHE
        .lock()
        .map_err(|e| anyhow::anyhow!("grammar cache lock poisoned: {e}"))?;

    // Fast path — already loaded.
    if let Some(entry) = cache.iter().find(|(id, _)| *id == lang) {
        return Ok(entry.1.language.clone());
    }

    // Slow path — load from disk.
    let (library, language) = load_from_disk(lang)?;
    let language_clone = language.clone();
    cache.push((
        lang,
        LoadedGrammar {
            _library: library,
            language,
        },
    ));
    Ok(language_clone)
}

/// Construct the path to the compiled grammar shared library.
///
/// Naming must match [`build.rs`]:
/// - Unix: `lib{symbol}.so` / `lib{symbol}.dylib`
/// - Windows: `{symbol}.dll` (no `lib` prefix)
fn so_path(lang: LangId) -> PathBuf {
    let out_dir = PathBuf::from(env!("OUT_DIR"));
    let symbol = lang.so_name();
    if cfg!(target_os = "macos") {
        out_dir.join(format!("lib{symbol}.dylib"))
    } else if cfg!(target_os = "windows") {
        out_dir.join(format!("{symbol}.dll"))
    } else {
        out_dir.join(format!("lib{symbol}.so"))
    }
}

/// Load a grammar `.so` and return the (Library, Language) pair.
fn load_from_disk(lang: LangId) -> Result<(Library, Language)> {
    let path = so_path(lang);
    let library = unsafe {
        Library::new(&path)
            .with_context(|| format!("failed to dlopen grammar: {}", path.display()))?
    };

    let symbol_name = lang.symbol_name();
    let func: libloading::Symbol<unsafe extern "C" fn() -> *const ()> = unsafe {
        library
            .get(symbol_name.as_bytes())
            .with_context(|| format!("symbol `{}` not found in {}", symbol_name, path.display()))?
    };

    let language = {
        let lang_fn = unsafe { LanguageFn::from_raw(*func) };
        Language::new(lang_fn)
    };

    Ok((library, language))
}
