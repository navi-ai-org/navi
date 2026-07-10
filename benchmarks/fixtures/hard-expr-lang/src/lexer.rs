use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Num(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    LParen,
    RParen,
}

#[derive(Debug)]
pub struct LexError(pub String);

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' => {
                chars.next();
            }
            '+' => {
                chars.next();
                out.push(Token::Plus);
            }
            '-' => {
                chars.next();
                out.push(Token::Minus);
            }
            '*' => {
                chars.next();
                out.push(Token::Star);
            }
            '/' => {
                chars.next();
                out.push(Token::Slash);
            }
            '^' => {
                chars.next();
                out.push(Token::Caret);
            }
            '(' => {
                chars.next();
                out.push(Token::LParen);
            }
            ')' => {
                chars.next();
                out.push(Token::RParen);
            }
            '0'..='9' | '.' => {
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() || d == '.' {
                        s.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let n: f64 = s
                    .parse()
                    .map_err(|_| LexError(format!("bad number {s}")))?;
                out.push(Token::Num(n));
            }
            other => return Err(LexError(format!("unexpected char {other}"))),
        }
    }
    Ok(out)
}
