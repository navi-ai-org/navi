use crate::{BinOp, Expr};
use std::fmt;

#[derive(Debug)]
pub struct EvalError(pub String);

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn eval(expr: &Expr) -> Result<f64, EvalError> {
    match expr {
        Expr::Num(n) => Ok(*n),
        Expr::UnaryNeg(e) => Ok(-eval(e)?),
        Expr::BinOp { op, left, right } => {
            let l = eval(left)?;
            let r = eval(right)?;
            match op {
                BinOp::Add => Ok(l + r),
                BinOp::Sub => Ok(l - r),
                BinOp::Mul => Ok(l * r),
                BinOp::Div => {
                    if r == 0.0 {
                        return Err(EvalError("div by zero".into()));
                    }
                    Ok(l / r)
                }
                BinOp::Pow => Ok(l.powf(r)),
            }
        }
    }
}
