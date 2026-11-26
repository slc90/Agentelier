use bevy::prelude::Result;
use std::{
    f64::consts::{E, PI, TAU},
    io,
};

const MAX_EXPRESSION_BYTES: usize = 512;
const MAX_PARSE_DEPTH: usize = 64;

pub(super) struct Expression(Node);

enum Node {
    Literal(f64),
    Constant(Constant),
    Variable { index: usize, name: String },
    Negative(Box<Node>),
    Binary(Binary, Box<Node>, Box<Node>),
    UnaryCall(UnaryFn, Box<Node>),
    BinaryCall(BinaryFn, Box<Node>, Box<Node>),
}

#[derive(Clone, Copy)]
enum Binary {
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
}

#[derive(Clone, Copy)]
enum Constant {
    Pi,
    Tau,
    Euler,
}

#[derive(Clone, Copy)]
enum UnaryFn {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Sqrt,
    Abs,
    Exp,
    Ln,
    Log10,
    Floor,
    Ceil,
    Round,
}

#[derive(Clone, Copy)]
enum BinaryFn {
    Min,
    Max,
    Atan2,
}

#[derive(Clone, Copy)]
enum Token<'a> {
    Number(f64),
    Name(&'a str),
    Operator(Binary),
    Open,
    Close,
    Comma,
}

struct Parser<'a, 'b> {
    tokens: Vec<Token<'a>>,
    position: usize,
    variables: &'b [String],
}

impl Expression {
    pub(super) fn parse(input: &str, variables: &[String]) -> Result<Self> {
        if input.is_empty() || input.len() > MAX_EXPRESSION_BYTES {
            return Err(io::Error::other("表达式必须非空且最多 512 字节").into());
        }
        let mut parser = Parser {
            tokens: lex(input)?,
            position: 0,
            variables,
        };
        let root = parser.expression(0, 0)?;
        if parser.peek().is_some() {
            return Err(parser.syntax("表达式后还有多余记号"));
        }
        Ok(Self(root))
    }

    pub(super) fn evaluate(&self, variables: &[f64]) -> Option<f64> {
        self.0.evaluate(variables)
    }

    pub(super) fn tex(&self, variables: &[Option<f64>]) -> String {
        self.0.tex(variables, 0)
    }
}

pub(super) fn valid_parameter(name: &str) -> bool {
    name.len() <= 16
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && Constant::parse(name).is_none()
        && UnaryFn::parse(name).is_none()
        && BinaryFn::parse(name).is_none()
}

fn lex(input: &str) -> Result<Vec<Token<'_>>> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        let start = at;
        at += 1;
        let token = match byte {
            b'+' => Token::Operator(Binary::Add),
            b'-' => Token::Operator(Binary::Subtract),
            b'*' => Token::Operator(Binary::Multiply),
            b'/' => Token::Operator(Binary::Divide),
            b'^' => Token::Operator(Binary::Power),
            b'(' => Token::Open,
            b')' => Token::Close,
            b',' => Token::Comma,
            byte if byte.is_ascii_whitespace() => continue,
            byte if byte.is_ascii_alphabetic() => {
                at += bytes[at..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_alphanumeric())
                    .count();
                Token::Name(&input[start..at])
            }
            b'0'..=b'9' | b'.' => {
                at += bytes[at..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_digit() || **byte == b'.')
                    .count();
                if matches!(bytes.get(at), Some(b'e' | b'E')) {
                    at += 1;
                    at += usize::from(matches!(bytes.get(at), Some(b'+' | b'-')));
                    at += bytes[at..]
                        .iter()
                        .take_while(|byte| byte.is_ascii_digit())
                        .count();
                }
                let number = input[start..at]
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite())
                    .ok_or_else(|| {
                        io::Error::other(format!("第 {} 个字节附近的数值格式无效", start + 1))
                    })?;
                Token::Number(number)
            }
            _ => {
                return Err(
                    io::Error::other(format!("第 {} 个字节附近有不支持的字符", start + 1)).into(),
                );
            }
        };
        tokens.push(token);
    }
    Ok(tokens)
}

impl Parser<'_, '_> {
    fn peek(&self) -> Option<Token<'_>> {
        self.tokens.get(self.position).copied()
    }

    fn take(&mut self) -> Option<Token<'_>> {
        let at = self.position;
        self.position += 1;
        self.tokens.get(at).copied()
    }

    fn expression(&mut self, minimum: u8, depth: usize) -> Result<Node> {
        if depth >= MAX_PARSE_DEPTH {
            return Err(self.syntax("表达式嵌套过深"));
        }
        let mut left = match self.take() {
            Some(Token::Number(value)) => Node::Literal(value),
            Some(Token::Name(name)) => {
                let name = name.to_owned();
                self.named(name, depth + 1)?
            }
            Some(Token::Open) => {
                let value = self.expression(0, depth + 1)?;
                self.close()?;
                value
            }
            Some(Token::Operator(Binary::Add)) => self.expression(5, depth + 1)?,
            Some(Token::Operator(Binary::Subtract)) => {
                Node::Negative(Box::new(self.expression(5, depth + 1)?))
            }
            _ => return Err(self.syntax("此处需要数字、变量、函数或括号")),
        };
        while let Some(Token::Operator(operator)) = self.peek() {
            let (left_power, right_power) = operator.precedence();
            if left_power < minimum {
                break;
            }
            self.position += 1;
            let right = self.expression(right_power, depth + 1)?;
            left = Node::Binary(operator, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn named(&mut self, name: String, depth: usize) -> Result<Node> {
        if matches!(self.peek(), Some(Token::Open)) {
            self.position += 1;
            if let Some(function) = UnaryFn::parse(&name) {
                let value = self.expression(0, depth)?;
                self.close()?;
                return Ok(Node::UnaryCall(function, Box::new(value)));
            }
            if let Some(function) = BinaryFn::parse(&name) {
                let first = self.expression(0, depth)?;
                if !matches!(self.take(), Some(Token::Comma)) {
                    return Err(self.syntax("此函数需要两个以逗号分隔的参数"));
                }
                let second = self.expression(0, depth)?;
                self.close()?;
                return Ok(Node::BinaryCall(
                    function,
                    Box::new(first),
                    Box::new(second),
                ));
            }
            return Err(self.syntax(&format!("未知函数 {name}")));
        }
        if let Some(constant) = Constant::parse(&name) {
            return Ok(Node::Constant(constant));
        }
        let index = self
            .variables
            .iter()
            .position(|variable| *variable == name)
            .ok_or_else(|| self.syntax(&format!("未定义变量 {name}")))?;
        Ok(Node::Variable { index, name })
    }

    fn close(&mut self) -> Result {
        if !matches!(self.take(), Some(Token::Close)) {
            return Err(self.syntax("此处需要右括号"));
        }
        Ok(())
    }

    fn syntax(&self, message: &str) -> bevy::prelude::BevyError {
        io::Error::other(format!("{message}（记号 {}）", self.position.max(1))).into()
    }
}

impl Node {
    fn evaluate(&self, variables: &[f64]) -> Option<f64> {
        let value = match self {
            Self::Literal(value) => *value,
            Self::Constant(value) => value.number(),
            Self::Variable { index, .. } => *variables.get(*index)?,
            Self::Negative(operand) => -operand.evaluate(variables)?,
            Self::Binary(operator, left, right) => {
                operator.evaluate(left.evaluate(variables)?, right.evaluate(variables)?)
            }
            Self::UnaryCall(function, argument) => function.evaluate(argument.evaluate(variables)?),
            Self::BinaryCall(function, left, right) => {
                function.evaluate(left.evaluate(variables)?, right.evaluate(variables)?)
            }
        };
        value.is_finite().then_some(value)
    }

    fn tex(&self, variables: &[Option<f64>], minimum: u8) -> String {
        match self {
            Self::Literal(value) => tex_number(*value, minimum),
            Self::Constant(value) => value.tex().into(),
            Self::Variable { index, name } => variables.get(*index).copied().flatten().map_or_else(
                || {
                    if name.len() == 1 {
                        name.clone()
                    } else {
                        format!("\\mathrm{{{name}}}")
                    }
                },
                |value| tex_number(value, minimum),
            ),
            Self::Negative(operand) => group(format!("-{}", operand.tex(variables, 5)), 5, minimum),
            Self::Binary(Binary::Divide, a, b) => format!(
                "\\frac{{{}}}{{{}}}",
                a.tex(variables, 0),
                b.tex(variables, 0)
            ),
            Self::Binary(Binary::Power, a, b) => group(
                format!("{}^{{{}}}", a.tex(variables, 7), b.tex(variables, 0)),
                6,
                minimum,
            ),
            Self::Binary(operator, a, b) => {
                let (power, right) = operator.precedence();
                let symbol = match operator {
                    Binary::Add => "+",
                    Binary::Subtract => "-",
                    _ => "\\cdot ",
                };
                group(
                    format!(
                        "{}{symbol}{}",
                        a.tex(variables, power),
                        b.tex(variables, right)
                    ),
                    power,
                    minimum,
                )
            }
            Self::UnaryCall(function, argument) => {
                let argument = argument.tex(variables, 0);
                match function {
                    UnaryFn::Sqrt => format!("\\sqrt{{{argument}}}"),
                    UnaryFn::Abs => format!("\\left|{argument}\\right|"),
                    _ => format!("\\mathrm{{{}}}\\left({argument}\\right)", function.name()),
                }
            }
            Self::BinaryCall(function, a, b) => format!(
                "\\mathrm{{{}}}\\left({},{}\\right)",
                function.name(),
                a.tex(variables, 0),
                b.tex(variables, 0)
            ),
        }
    }
}

fn group(text: String, strength: u8, minimum: u8) -> String {
    if strength < minimum {
        format!("\\left({text}\\right)")
    } else {
        text
    }
}

pub(super) fn tex_number(value: f64, minimum: u8) -> String {
    let text = if value == 0.0 {
        "0".into()
    } else if (1e-5..1e7).contains(&value.abs()) {
        value.to_string()
    } else {
        let scientific = format!("{value:e}");
        scientific.split_once('e').map_or_else(
            || scientific.clone(),
            |(m, e)| format!("{m}\\times 10^{{{e}}}"),
        )
    };
    if value < 0.0 {
        group(text, 5, minimum.max(6))
    } else {
        text
    }
}

impl Binary {
    fn precedence(self) -> (u8, u8) {
        match self {
            Self::Add | Self::Subtract => (1, 2),
            Self::Multiply | Self::Divide => (3, 4),
            Self::Power => (6, 5),
        }
    }

    fn evaluate(self, a: f64, b: f64) -> f64 {
        match self {
            Self::Add => a + b,
            Self::Subtract => a - b,
            Self::Multiply => a * b,
            Self::Divide => a / b,
            Self::Power => a.powf(b),
        }
    }
}

impl Constant {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "pi" => Self::Pi,
            "tau" => Self::Tau,
            "e" => Self::Euler,
            _ => return None,
        })
    }
    fn number(self) -> f64 {
        match self {
            Self::Pi => PI,
            Self::Tau => TAU,
            Self::Euler => E,
        }
    }
    fn tex(self) -> &'static str {
        match self {
            Self::Pi => "\\pi",
            Self::Tau => "\\tau",
            Self::Euler => "e",
        }
    }
}

impl UnaryFn {
    const NAMES: [(&'static str, Self); 17] = [
        ("sin", Self::Sin),
        ("cos", Self::Cos),
        ("tan", Self::Tan),
        ("asin", Self::Asin),
        ("acos", Self::Acos),
        ("atan", Self::Atan),
        ("sinh", Self::Sinh),
        ("cosh", Self::Cosh),
        ("tanh", Self::Tanh),
        ("sqrt", Self::Sqrt),
        ("abs", Self::Abs),
        ("exp", Self::Exp),
        ("ln", Self::Ln),
        ("log10", Self::Log10),
        ("floor", Self::Floor),
        ("ceil", Self::Ceil),
        ("round", Self::Round),
    ];

    fn parse(name: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
    }
    fn name(self) -> &'static str {
        match self {
            Self::Sin => "sin",
            Self::Cos => "cos",
            Self::Tan => "tan",
            Self::Asin => "asin",
            Self::Acos => "acos",
            Self::Atan => "atan",
            Self::Sinh => "sinh",
            Self::Cosh => "cosh",
            Self::Tanh => "tanh",
            Self::Sqrt => "sqrt",
            Self::Abs => "abs",
            Self::Exp => "exp",
            Self::Ln => "ln",
            Self::Log10 => "log10",
            Self::Floor => "floor",
            Self::Ceil => "ceil",
            Self::Round => "round",
        }
    }
    fn evaluate(self, value: f64) -> f64 {
        match self {
            Self::Sin => value.sin(),
            Self::Cos => value.cos(),
            Self::Tan => value.tan(),
            Self::Asin => value.asin(),
            Self::Acos => value.acos(),
            Self::Atan => value.atan(),
            Self::Sinh => value.sinh(),
            Self::Cosh => value.cosh(),
            Self::Tanh => value.tanh(),
            Self::Sqrt => value.sqrt(),
            Self::Abs => value.abs(),
            Self::Exp => value.exp(),
            Self::Ln => value.ln(),
            Self::Log10 => value.log10(),
            Self::Floor => value.floor(),
            Self::Ceil => value.ceil(),
            Self::Round => value.round(),
        }
    }
}

impl BinaryFn {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "min" => Self::Min,
            "max" => Self::Max,
            "atan2" => Self::Atan2,
            _ => return None,
        })
    }
    fn name(self) -> &'static str {
        match self {
            Self::Min => "min",
            Self::Max => "max",
            Self::Atan2 => "atan2",
        }
    }
    fn evaluate(self, a: f64, b: f64) -> f64 {
        match self {
            Self::Min => a.min(b),
            Self::Max => a.max(b),
            Self::Atan2 => a.atan2(b),
        }
    }
}
