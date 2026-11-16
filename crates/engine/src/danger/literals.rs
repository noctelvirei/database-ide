//! Exact literal equality without executing expressions or rounding SQL numbers.

use sqlparser::ast::{Expr, UnaryOperator, Value};

/// Compares only literal values; NULL, parameters and column references cannot establish truth.
pub(super) fn equal(left: &Expr, right: &Expr) -> bool {
    if let (Some(left), Some(right)) = (number(left), number(right)) {
        return left == right;
    }
    match (value(left), value(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn value(expr: &Expr) -> Option<&Value> {
    match expr {
        Expr::Nested(expr) => value(expr),
        Expr::Value(value) => match &value.value {
            Value::Null | Value::Placeholder(_) => None,
            value => Some(value),
        },
        _ => None,
    }
}

#[derive(PartialEq, Eq)]
struct Number {
    negative: bool,
    digits: String,
    exponent: i128,
}

fn number(expr: &Expr) -> Option<Number> {
    match expr {
        Expr::Nested(expr) => number(expr),
        Expr::UnaryOp {
            op: UnaryOperator::Plus,
            expr,
        } => number(expr),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => {
            let mut number = number(expr)?;
            number.negative = !number.negative && number.digits != "0";
            Some(number)
        }
        Expr::Value(value) => {
            let Value::Number(text, _) = &value.value else {
                return None;
            };
            let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
            let exponent: i128 = exponent.parse().ok()?;
            let fractional = mantissa
                .split_once('.')
                .map(|(_, fraction)| fraction.len())
                .unwrap_or(0);
            let digits = mantissa.replace('.', "");
            let digits = digits.trim_start_matches('0');
            if digits.is_empty() {
                return Some(Number {
                    negative: false,
                    digits: "0".to_owned(),
                    exponent: 0,
                });
            }
            let significant = digits.trim_end_matches('0');
            let exponent = exponent
                .checked_sub(fractional as i128)?
                .checked_add((digits.len() - significant.len()) as i128)?;
            Some(Number {
                negative: false,
                digits: significant.to_owned(),
                exponent,
            })
        }
        _ => None,
    }
}
