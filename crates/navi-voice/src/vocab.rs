//! SentencePiece-style vocabulary loaded from `vocab.txt` (one piece per line).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Vocabulary with blank id = last piece (`<blank>`).
#[derive(Debug, Clone)]
pub struct Vocab {
    pieces: Vec<String>,
    blank_id: usize,
}

impl Vocab {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read vocab {}", path.display()))?;
        let pieces: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        if pieces.is_empty() {
            bail!("empty vocab at {}", path.display());
        }
        let blank_id = pieces
            .iter()
            .position(|p| p == "<blank>")
            .unwrap_or(pieces.len().saturating_sub(1));
        Ok(Self { pieces, blank_id })
    }

    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    pub fn blank_id(&self) -> usize {
        self.blank_id
    }

    pub fn piece(&self, id: usize) -> Option<&str> {
        self.pieces.get(id).map(|s| s.as_str())
    }

    /// Decode token ids to human-readable text (SentencePiece `▁` → space).
    /// Language tags like `<en-US>` are stripped.
    pub fn decode(&self, token_ids: &[usize]) -> String {
        let mut out = String::new();
        for &id in token_ids {
            let Some(piece) = self.piece(id) else {
                continue;
            };
            if is_lang_tag(piece) || piece == "<blank>" {
                continue;
            }
            if piece == "<unk>" {
                out.push('�');
                continue;
            }
            if let Some(rest) = piece.strip_prefix('▁') {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(rest);
            } else {
                out.push_str(piece);
            }
        }
        // Collapse accidental double spaces from empty `▁` pieces.
        let mut cleaned = String::with_capacity(out.len());
        let mut prev_space = false;
        for ch in out.chars() {
            if ch == ' ' {
                if !prev_space && !cleaned.is_empty() {
                    cleaned.push(' ');
                }
                prev_space = true;
            } else {
                cleaned.push(ch);
                prev_space = false;
            }
        }
        cleaned.trim().to_string()
    }
}

/// Detect SentencePiece pieces that encode a language tag like `<en-US>`.
pub fn is_lang_tag(piece: &str) -> bool {
    let bytes = piece.as_bytes();
    if bytes.len() < 4 || bytes[0] != b'<' || bytes[bytes.len() - 1] != b'>' {
        return false;
    }
    let inner = &bytes[1..bytes.len() - 1];
    match inner.len() {
        2 => inner[0].is_ascii_lowercase() && inner[1].is_ascii_lowercase(),
        5 => {
            inner[0].is_ascii_lowercase()
                && inner[1].is_ascii_lowercase()
                && inner[2] == b'-'
                && inner[3].is_ascii_uppercase()
                && inner[4].is_ascii_uppercase()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn decode_sentencepiece_pieces() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "<unk>").unwrap();
        writeln!(f, "▁").unwrap();
        writeln!(f, "he").unwrap();
        writeln!(f, "llo").unwrap();
        writeln!(f, "▁world").unwrap();
        writeln!(f, "<en-US>").unwrap();
        writeln!(f, "<blank>").unwrap();
        let v = Vocab::load(f.path()).unwrap();
        assert_eq!(v.blank_id(), 6);
        // tokens: <en-US>, he, llo, ▁world
        let text = v.decode(&[5, 2, 3, 4]);
        assert_eq!(text, "hello world");
    }
}
