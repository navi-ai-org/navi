use crate::{BinOp, Expr};
use crate::lexer::Token;
use std::fmt;

#[derive(Debug)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Pratt / precedence-climb style parser with several intentional mistakes.
pub fn parse(tokens: &[Token]) -> Result<Expr, ParseError> {
    let mut p = Parser { tokens, i: 0 };
    let expr = p.parse_expr(0)?;
    if p.i != tokens.len() {
        return Err(ParseError("trailing tokens".into()));
    }
    Ok(expr)
}

struct Parser<'a> {
    tokens: &'a [Token],
    i: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.i)
    }

    fn bump(&mut self) -> Option<&'a Token> {
        let t = self.tokens.get(self.i);
        if t.is_some() {
            self.i += 1;
        }
        t
    }

    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_prefix()?;

        loop {
            let op = match self.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                Some(Token::Caret) => BinOp::Pow,
                _ => break,
            };
            let (lbp, rbp) = binding_power(op);
            // TODO(fix): uses wrong side for min_bp compare for right-assoc
            if lbp < min_bp {
                break;
            }
            self.bump();
            // TODO(fix): always uses lbp+1 (left-assoc) even for ^
            let rhs = self.parse_expr(lbp + 1)?;
            let _ = rbp;
            lhs = Expr::BinOp {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, ParseError> {
        match self.bump() {
            Some(Token::Num(n)) => Ok(Expr::Num(*n)),
            Some(Token::Minus) => {
                // unary minus — bind tighter than ^ incorrectly low
                let e = self.parse_expr(binding_power(BinOp::Pow).0)?; // TODO(fix): should be > pow
                Ok(Expr::UnaryNeg(Box::new(e)))
            }
            Some(Token::LParen) => {
                let e = self.parse_expr(0)?;
                match self.bump() {
                    Some(Token::RParen) => Ok(e),
                    _ => Err(ParseError("expected )".into())),
                }
            }
            other => Err(ParseError(format!("unexpected {other:?}"))),
        }
    }
}

fn binding_power(op: BinOp) -> (u8, u8) {
    match op {
        // TODO(fix): + and * precedence swapped relative to intended grammar
        BinOp::Add | BinOp::Sub => (3, 4),
        BinOp::Mul | BinOp::Div => (1, 2),
        // intended: pow right-assoc high; currently left-ish low-ish
        BinOp::Pow => (5, 6), // both sides left-assoc pattern when used with lbp+1
    }
}
