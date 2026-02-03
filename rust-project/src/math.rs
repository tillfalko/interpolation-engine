use crate::interp::{get_interpdata, interpolate_inserts, value_to_string};
use crate::model::ProgramLoadContext;
use anyhow::{anyhow, Result};
use serde_json::{Map, Value};

const LEGAL: &str = " .0123456789+-*/%^(),_";

pub fn eval_math(inserts: &Map<String, Value>, input: &str, ctx: &ProgramLoadContext) -> Result<i64> {
    let interpolated = interpolate_inserts(inserts, input, ctx)?;
    let mut expr = value_to_string(&interpolated);

    if expr
        .chars()
        .any(|c| !LEGAL.contains(c) && !c.is_ascii_alphabetic())
    {
        return Err(anyhow!("Math expression contains illegal characters: {expr}"));
    }
    if expr.matches('(').count() != expr.matches(')').count() {
        return Err(anyhow!("Illegal parentheses in math input '{expr}'"));
    }

    while let Some((start, end)) = find_innermost_parens(&expr) {
        let inner = &expr[start + 1..end];
        let (fn_name, fn_start) = find_function_name(&expr, start);
        let value = if let Some(name) = fn_name {
            eval_function(inserts, &name, inner, ctx)?
        } else {
            eval_arithmetic(inner)?
        };
        let prefix = &expr[..fn_start];
        let suffix = &expr[end + 1..];
        expr = format!("{prefix}{value}{suffix}");
    }

    let value = eval_arithmetic(&expr)?;
    let rounded = value.round();
    if value != 0.0 && ((rounded - value).abs() / value.abs()) >= 0.0001 {
        return Err(anyhow!(
            "Math result '{value}' is not an integer within tolerance"
        ));
    }
    Ok(rounded as i64)
}

fn find_innermost_parens(s: &str) -> Option<(usize, usize)> {
    let mut last_open = None;
    for (i, ch) in s.char_indices() {
        if ch == '(' {
            last_open = Some(i);
        } else if ch == ')' {
            if let Some(start) = last_open {
                return Some((start, i));
            }
        }
    }
    None
}

fn find_function_name(s: &str, paren_index: usize) -> (Option<String>, usize) {
    let bytes = s.as_bytes();
    if paren_index == 0 {
        return (None, paren_index);
    }
    let mut i = paren_index;
    while i > 0 {
        let c = bytes[i - 1] as char;
        if c.is_alphanumeric() || c == '_' {
            i -= 1;
        } else {
            break;
        }
    }
    if i < paren_index {
        let name = s[i..paren_index].to_string();
        return (Some(name), i);
    }
    (None, paren_index)
}

fn eval_function(
    inserts: &Map<String, Value>,
    name: &str,
    inner: &str,
    ctx: &ProgramLoadContext,
) -> Result<f64> {
    match name {
        "length" => {
            let v = get_interpdata(inserts, inner, ctx)?;
            let arr = v
                .as_array()
                .ok_or_else(|| anyhow!("length() expects a list, got {v:?}"))?;
            Ok(arr.len() as f64)
        }
        "min" => eval_min_max(inserts, inner, ctx, true),
        "max" => eval_min_max(inserts, inner, ctx, false),
        "round" => Ok(eval_arithmetic(inner)?.round()),
        "sign" => {
            let v = eval_arithmetic(inner)?;
            Ok(if v > 0.0 { 1.0 } else if v < 0.0 { -1.0 } else { 0.0 })
        }
        _ => Err(anyhow!("Unknown math function '{name}'")),
    }
}

fn eval_min_max(
    inserts: &Map<String, Value>,
    inner: &str,
    ctx: &ProgramLoadContext,
    is_min: bool,
) -> Result<f64> {
    let numeric = inner.chars().all(|c| " .0123456789+-*/%^,".contains(c));
    if numeric {
        let mut nums = Vec::new();
        for part in inner.split(',') {
            if part.trim().is_empty() {
                continue;
            }
            nums.push(eval_arithmetic(part)?);
        }
        if nums.is_empty() {
            return Err(anyhow!("min/max requires at least one value"));
        }
        return Ok(if is_min {
            nums.into_iter().fold(f64::INFINITY, f64::min)
        } else {
            nums.into_iter().fold(f64::NEG_INFINITY, f64::max)
        });
    }

    let v = get_interpdata(inserts, inner, ctx)?;
    let arr = v
        .as_array()
        .ok_or_else(|| anyhow!("min/max expects a list, got {v:?}"))?;
    if arr.is_empty() {
        return Err(anyhow!("min/max list is empty"));
    }
    let mut nums = Vec::new();
    for val in arr {
        match val {
            Value::Number(n) => nums.push(n.as_f64().unwrap_or(0.0)),
            _ => return Err(anyhow!("min/max list must contain numbers")),
        }
    }
    Ok(if is_min {
        nums.into_iter().fold(f64::INFINITY, f64::min)
    } else {
        nums.into_iter().fold(f64::NEG_INFINITY, f64::max)
    })
}

#[derive(Debug, Clone)]
enum Token {
    Number(f64),
    Op(char),
}

fn eval_arithmetic(expr: &str) -> Result<f64> {
    let tokens = tokenize(expr)?;
    let rpn = to_rpn(&tokens)?;
    eval_rpn(&rpn)
}

fn tokenize(expr: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = expr.chars().peekable();
    let mut last_was_op = true;
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if "+-*/%^".contains(ch) {
            chars.next();
            if ch == '-' && last_was_op {
                let mut num = String::from("-");
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() || c == '.' {
                        num.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let value: f64 = num.parse()?;
                tokens.push(Token::Number(value));
                last_was_op = false;
                continue;
            }
            tokens.push(Token::Op(ch));
            last_was_op = true;
            continue;
        }
        if ch.is_ascii_digit() || ch == '.' {
            let mut num = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() || c == '.' {
                    num.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            let value: f64 = num.parse()?;
            tokens.push(Token::Number(value));
            last_was_op = false;
            continue;
        }
        return Err(anyhow!("Unexpected character in math: '{ch}'"));
    }
    Ok(tokens)
}

fn precedence(op: char) -> i32 {
    match op {
        '^' => 4,
        '*' | '/' | '%' => 3,
        '+' | '-' => 2,
        _ => 0,
    }
}

fn to_rpn(tokens: &[Token]) -> Result<Vec<Token>> {
    let mut output = Vec::new();
    let mut ops: Vec<char> = Vec::new();
    for token in tokens {
        match token {
            Token::Number(_) => output.push(token.clone()),
            Token::Op(op) => {
                while let Some(&top) = ops.last() {
                    if precedence(top) >= precedence(*op) {
                        output.push(Token::Op(top));
                        ops.pop();
                    } else {
                        break;
                    }
                }
                ops.push(*op);
            }
        }
    }
    while let Some(op) = ops.pop() {
        output.push(Token::Op(op));
    }
    Ok(output)
}

fn eval_rpn(tokens: &[Token]) -> Result<f64> {
    let mut stack: Vec<f64> = Vec::new();
    for token in tokens {
        match token {
            Token::Number(n) => stack.push(*n),
            Token::Op(op) => {
                let b = stack.pop().ok_or_else(|| anyhow!("Math stack underflow"))?;
                let a = stack.pop().ok_or_else(|| anyhow!("Math stack underflow"))?;
                let v = match op {
                    '+' => a + b,
                    '-' => a - b,
                    '*' => a * b,
                    '/' => a / b,
                    '%' => a % b,
                    '^' => a.powf(b),
                    _ => return Err(anyhow!("Unknown operator '{op}'")),
                };
                stack.push(v);
            }
        }
    }
    if stack.len() != 1 {
        return Err(anyhow!("Math expression failed to reduce"));
    }
    Ok(stack[0])
}
