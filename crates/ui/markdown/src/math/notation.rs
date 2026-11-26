use latex_rust::{MathNode, ParseOptions, Token};
use std::borrow::Cow;

const MAX_EXPANSION_DEPTH: usize = 32;
const MAX_EXPANDED_BYTES: usize = 64 * 1024;

struct Macro {
    name: &'static str,
    arguments: usize,
    template: &'static str,
}

impl Macro {
    const fn new(name: &'static str, arguments: usize, template: &'static str) -> Self {
        Self {
            name,
            arguments,
            template,
        }
    }
}

const MACROS: &[Macro] = &[
    Macro::new("abs", 1, r"\left|#1\right|"),
    Macro::new("norm", 1, r"\left\Vert #1\right\Vert"),
    Macro::new("set", 1, r"\left\{#1\right\}"),
    Macro::new("bra", 1, r"\left\langle #1\right|"),
    Macro::new("ket", 1, r"\left|#1\right\rangle"),
    Macro::new("braket", 1, r"\left\langle #1\right\rangle"),
    Macro::new(
        "ketbra",
        2,
        r"\left|#1\right\rangle\!\left\langle #2\right|",
    ),
    Macro::new("dv", 2, r"\frac{\mathrm{d}#1}{\mathrm{d}#2}"),
    Macro::new("pdv", 2, r"\frac{\partial #1}{\partial #2}"),
    Macro::new("mathbbm", 1, r"\mathbb{#1}"),
    Macro::new("qty", 2, r"{#1}\,{#2}"),
    Macro::new("SI", 2, r"{#1}\,{#2}"),
    Macro::new("unit", 1, r"{#1}"),
    Macro::new("si", 1, r"{#1}"),
    Macro::new("per", 0, "/"),
    Macro::new("squared", 0, "^{2}"),
    Macro::new("cubed", 0, "^{3}"),
    Macro::new("metre", 0, r"\mathrm{m}"),
    Macro::new("meter", 0, r"\mathrm{m}"),
    Macro::new("second", 0, r"\mathrm{s}"),
    Macro::new("kilogram", 0, r"\mathrm{kg}"),
    Macro::new("ampere", 0, r"\mathrm{A}"),
    Macro::new("kelvin", 0, r"\mathrm{K}"),
    Macro::new("mole", 0, r"\mathrm{mol}"),
    Macro::new("newton", 0, r"\mathrm{N}"),
    Macro::new("joule", 0, r"\mathrm{J}"),
    Macro::new("hertz", 0, r"\mathrm{Hz}"),
    Macro::new("volt", 0, r"\mathrm{V}"),
    Macro::new("watt", 0, r"\mathrm{W}"),
    Macro::new("pascal", 0, r"\mathrm{Pa}"),
    Macro::new("degreeCelsius", 0, r"{}^{\circ}\!\mathrm{C}"),
];

pub(super) fn parse(input: &str) -> Result<MathNode, String> {
    let source = expand(input)?;
    let options = ParseOptions::new().with_unicode_symbols(true);
    latex_rust::parse_with_options(&source, &options)
        .map(|(ast, _)| ast)
        .map_err(|error| error.to_string())
}

fn expand(input: &str) -> Result<Cow<'_, str>, String> {
    if !input.contains('\\') {
        return Ok(Cow::Borrowed(input));
    }
    let tokens = latex_rust::tokenize(input).map_err(|error| error.to_string())?;
    if !tokens.iter().any(|token| rule(token).is_some()) {
        return Ok(Cow::Borrowed(input));
    }
    let mut output = String::with_capacity(input.len());
    expand_tokens(&tokens, &mut output, 0)?;
    Ok(Cow::Owned(output))
}

fn rule(token: &Token) -> Option<&'static Macro> {
    let Token::Command(command) = token else {
        return None;
    };
    MACROS.iter().find(|definition| command == definition.name)
}

fn expand_tokens(tokens: &[Token], output: &mut String, depth: usize) -> Result<(), String> {
    if depth >= MAX_EXPANSION_DEPTH {
        return Err("公式宏嵌套过深".into());
    }
    let mut tokens = tokens;
    while let Some((token, rest)) = tokens.split_first() {
        tokens = rest;
        if let Some(definition) = rule(token) {
            let mut arguments: [&[Token]; 2] = [&[], &[]];
            for argument in arguments.iter_mut().take(definition.arguments) {
                *argument = take_argument(&mut tokens).ok_or_else(|| {
                    format!(
                        "宏 \\{} 需要 {} 个花括号参数",
                        definition.name, definition.arguments
                    )
                })?;
            }
            let mut segments = definition.template.split('#');
            if let Some(literal) = segments.next() {
                output.push_str(literal);
            }
            for segment in segments {
                let (index, following) = if let Some(following) = segment.strip_prefix('1') {
                    (0, following)
                } else if let Some(following) = segment.strip_prefix('2') {
                    (1, following)
                } else {
                    return Err(format!("宏 \\{} 的参数模板无效", definition.name));
                };
                expand_tokens(arguments[index], output, depth + 1)?;
                output.push_str(following);
            }
            output.push(' ');
        } else {
            write_token(token, tokens.first(), output);
        }
        if output.len() > MAX_EXPANDED_BYTES {
            return Err("公式宏展开后过长".into());
        }
    }
    Ok(())
}

fn take_argument<'a>(tokens: &mut &'a [Token]) -> Option<&'a [Token]> {
    let start = tokens
        .iter()
        .position(|token| !matches!(token, Token::Space))?;
    let (Token::BeginGroup, rest) = tokens.get(start..)?.split_first()? else {
        return None;
    };
    let mut groups = 1;
    for (index, token) in rest.iter().enumerate() {
        match token {
            Token::BeginGroup => groups += 1,
            Token::EndGroup => {
                groups -= 1;
                if groups == 0 {
                    *tokens = &rest[index + 1..];
                    return Some(&rest[..index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn write_token(token: &Token, next: Option<&Token>, output: &mut String) {
    match token {
        Token::Char(character) => output.push(*character),
        Token::Command(command) => {
            output.push('\\');
            output.push_str(command);
            if command
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
                && matches!(next, Some(Token::Char(character)) if character.is_ascii_alphabetic())
            {
                output.push(' ');
            }
        }
        Token::BeginGroup => output.push('{'),
        Token::EndGroup => output.push('}'),
        Token::Superscript => output.push('^'),
        Token::Subscript => output.push('_'),
        Token::AlignmentTab => output.push('&'),
        Token::MathShift => output.push('$'),
        Token::DisplayShift => output.push_str("$$"),
        Token::Space => output.push(' '),
        Token::TextArg(text) => {
            output.push('{');
            output.push_str(text);
            output.push('}');
        }
    }
}
