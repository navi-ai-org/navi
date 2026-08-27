//! Language → prompt embedding index for multilingual Nemotron.
//!
//! Mirrors NVIDIA's `prompt_dictionary` (also used by parakeet-rs).
//! `lang_id` on the onnx-community export is this index (not a vocab token id).

/// Resolve a language string to the encoder `lang_id` prompt index.
///
/// Accepts `"auto"`, `"en"`, `"en-US"`, `"pt-BR"`, etc. Unknown values fall back
/// to `auto` (101).
pub fn resolve_lang_id(language: &str) -> i64 {
    let key = language.trim();
    if key.is_empty() || key.eq_ignore_ascii_case("auto") {
        return 101;
    }
    // Exact / common aliases first
    let lower = key.to_ascii_lowercase();
    for (code, idx) in PROMPT_DICTIONARY {
        if code.eq_ignore_ascii_case(key) || code.eq_ignore_ascii_case(&lower) {
            return *idx;
        }
    }
    // Try base language (en from en-US)
    if let Some((base, _)) = lower.split_once('-') {
        for (code, idx) in PROMPT_DICTIONARY {
            if code.eq_ignore_ascii_case(base) {
                return *idx;
            }
        }
    }
    101
}

/// Subset + full dictionary used by NVIDIA multilingual Nemotron 3.5.
const PROMPT_DICTIONARY: &[(&str, i64)] = &[
    ("auto", 101),
    ("en", 0),
    ("en-US", 0),
    ("en-GB", 1),
    ("es-ES", 2),
    ("es", 3),
    ("es-US", 3),
    ("zh-CN", 4),
    ("zh-TW", 5),
    ("hi", 6),
    ("hi-IN", 6),
    ("ar", 7),
    ("ar-AR", 7),
    ("fr", 8),
    ("fr-FR", 8),
    ("de", 9),
    ("de-DE", 9),
    ("ja-JP", 10),
    ("ru", 11),
    ("ru-RU", 11),
    ("pt-BR", 12),
    ("pt", 13),
    ("pt-PT", 13),
    ("ko", 14),
    ("ko-KR", 14),
    ("it", 15),
    ("it-IT", 15),
    ("nl", 16),
    ("nl-NL", 16),
    ("pl", 17),
    ("pl-PL", 17),
    ("tr", 18),
    ("tr-TR", 18),
    ("uk", 19),
    ("uk-UA", 19),
    ("ro", 20),
    ("ro-RO", 20),
    ("el", 21),
    ("el-GR", 21),
    ("cs", 22),
    ("cs-CZ", 22),
    ("hu", 23),
    ("hu-HU", 23),
    ("sv", 24),
    ("sv-SE", 24),
    ("da", 25),
    ("da-DK", 25),
    ("fi", 26),
    ("fi-FI", 26),
    ("no", 27),
    ("sk", 28),
    ("sk-SK", 28),
    ("hr", 29),
    ("hr-HR", 29),
    ("bg", 30),
    ("bg-BG", 30),
    ("lt", 31),
    ("lt-LT", 31),
    ("th-TH", 32),
    ("vi-VN", 33),
    ("id-ID", 34),
    ("ms-MY", 35),
    ("fr-CA", 100),
    ("nb", 103),
    ("nb-NO", 103),
    ("nn", 104),
    ("nn-NO", 104),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_resolve_common() {
        assert_eq!(resolve_lang_id("auto"), 101);
        assert_eq!(resolve_lang_id(""), 101);
        assert_eq!(resolve_lang_id("en-US"), 0);
        assert_eq!(resolve_lang_id("en"), 0);
        assert_eq!(resolve_lang_id("pt-BR"), 12);
        assert_eq!(resolve_lang_id("pt"), 13);
        assert_eq!(resolve_lang_id("unknown-xx"), 101);
    }
}
