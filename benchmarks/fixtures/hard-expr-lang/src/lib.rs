//! Tiny expression language: numbers, + - * / ^, parentheses, unary minus.
//! Grammar intended (highest precedence first):
//!   unary - 
//!   ^  (right-associative)
//!   * /
//!   + -  (left-associative)

mod lexer;
mod parser;
mod eval;

pub use eval::eval;
pub use lexer::{LexError, Token, tokenize};
pub use parser::{ParseError, parse};

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    UnaryNeg(Box<Expr>),
    BinOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

pub fn eval_str(input: &str) -> Result<f64, String> {
    let tokens = tokenize(input).map_err(|e| e.to_string())?;
    let expr = parse(&tokens).map_err(|e| e.to_string())?;
    eval(&expr).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn basic_arithmetic() {
        approx(eval_str("1+2*3").unwrap(), 7.0);
        approx(eval_str("(1+2)*3").unwrap(), 9.0);
    }

    #[test]
    fn power_right_assoc() {
        // 2^3^2 = 2^(3^2) = 512, not (2^3)^2 = 64
        approx(eval_str("2^3^2").unwrap(), 512.0);
    }

    #[test]
    fn unary_minus() {
        approx(eval_str("-3+5").unwrap(), 2.0);
        approx(eval_str("2*-3").unwrap(), -6.0);
        approx(eval_str("--4").unwrap(), 4.0);
    }

    #[test]
    fn division_and_subtraction_left_assoc() {
        approx(eval_str("10-3-2").unwrap(), 5.0);
        approx(eval_str("20/5/2").unwrap(), 2.0);
    }

    #[test]
    fn mixed() {
        approx(eval_str("2+3*4^2").unwrap(), 50.0);
        approx(eval_str("-2^2").unwrap(), -4.0); // unary applies to 2^2? standard: -(2^2) = -4
    }

    #[test]
    fn whitespace() {
        approx(eval_str("  12 +  8 / 4 ").unwrap(), 14.0);
    }

    #[test]
    fn div_by_zero_err() {
        assert!(eval_str("1/0").is_err());
    }
}
