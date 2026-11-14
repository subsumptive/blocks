use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub line: usize,
    pub col: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TokenKind {
    // Keywords
    Import,
    From,
    As,
    Block,
    Input,
    Output,
    Use,
    Pipeline,
    True,
    False,

    // Identifiers & Literals
    Ident(String),
    String(String),
    Integer(i64),
    Float(f64),

    // Operators & Punctuation
    Eq,     // =
    Colon,  // :
    Comma,  // ,
    LParen, // (
    RParen, // )
    LBrace, // {
    RBrace, // }
    Eof,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Error)]
pub enum LexError {
    #[error("unexpected character '{ch}' at {line}:{col}")]
    UnexpectedChar { ch: char, line: usize, col: usize },
    #[error("unterminated string literal at {line}:{col}")]
    UnterminatedString { line: usize, col: usize },
    #[error("invalid number literal '{text}' at {line}:{col}")]
    InvalidNumber {
        text: String,
        line: usize,
        col: usize,
    },
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut line = 1;
    let mut col = 1;

    while i < chars.len() {
        let ch = chars[i];

        if ch == ' ' || ch == '\t' || ch == '\r' {
            i += 1;
            col += 1;
            continue;
        }
        if ch == '\n' {
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        let start = i;
        let start_col = col;

        if ch == '"' {
            i += 1;
            col += 1;
            let mut s = String::new();
            let mut terminated = false;
            while i < chars.len() {
                let c = chars[i];
                if c == '"' {
                    i += 1;
                    col += 1;
                    terminated = true;
                    break;
                } else if c == '\\' && i + 1 < chars.len() {
                    let next = chars[i + 1];
                    match next {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        other => s.push(other),
                    }
                    i += 2;
                    col += 2;
                } else if c == '\n' {
                    return Err(LexError::UnterminatedString {
                        line,
                        col: start_col,
                    });
                } else {
                    s.push(c);
                    i += 1;
                    col += 1;
                }
            }
            if !terminated {
                return Err(LexError::UnterminatedString {
                    line,
                    col: start_col,
                });
            }
            tokens.push(Token {
                kind: TokenKind::String(s),
                span: Span {
                    line,
                    col: start_col,
                    start,
                    end: i,
                },
            });
            continue;
        }

        if ch.is_ascii_digit()
            || (ch == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let mut num_str = String::new();
            if ch == '-' {
                num_str.push('-');
                i += 1;
                col += 1;
            }
            let mut is_float = false;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                if chars[i] == '.' {
                    if is_float {
                        break;
                    }
                    if i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                        is_float = true;
                    } else {
                        break;
                    }
                }
                num_str.push(chars[i]);
                i += 1;
                col += 1;
            }
            let kind = if is_float {
                let val = num_str
                    .parse::<f64>()
                    .map_err(|_| LexError::InvalidNumber {
                        text: num_str.clone(),
                        line,
                        col: start_col,
                    })?;
                TokenKind::Float(val)
            } else {
                let val = num_str
                    .parse::<i64>()
                    .map_err(|_| LexError::InvalidNumber {
                        text: num_str.clone(),
                        line,
                        col: start_col,
                    })?;
                TokenKind::Integer(val)
            };
            tokens.push(Token {
                kind,
                span: Span {
                    line,
                    col: start_col,
                    start,
                    end: i,
                },
            });
            continue;
        }

        if ch.is_ascii_alphabetic() || ch == '_' {
            let mut ident = String::new();
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                ident.push(chars[i]);
                i += 1;
                col += 1;
            }
            let kind = match ident.as_str() {
                "import" => TokenKind::Import,
                "from" => TokenKind::From,
                "as" => TokenKind::As,
                "block" => TokenKind::Block,
                "input" => TokenKind::Input,
                "output" => TokenKind::Output,
                "use" => TokenKind::Use,
                "pipeline" => TokenKind::Pipeline,
                "true" => TokenKind::True,
                "false" => TokenKind::False,
                _ => TokenKind::Ident(ident),
            };
            tokens.push(Token {
                kind,
                span: Span {
                    line,
                    col: start_col,
                    start,
                    end: i,
                },
            });
            continue;
        }

        let single = match ch {
            '=' => TokenKind::Eq,
            ':' => TokenKind::Colon,
            ',' => TokenKind::Comma,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            _ => {
                return Err(LexError::UnexpectedChar {
                    ch,
                    line,
                    col: start_col,
                })
            }
        };
        i += 1;
        col += 1;
        tokens.push(Token {
            kind: single,
            span: Span {
                line,
                col: start_col,
                start,
                end: i,
            },
        });
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span {
            line,
            col,
            start: i,
            end: i,
        },
    });
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_greeting_block() {
        let src = r#"
// Greeting block
block Greeting {
    input name: Text
    output message: Text
    use formatter = Format("Hello, {}")
    pipeline {
        message = formatter(name)
    }
}
"#;
        let tokens = tokenize(src).unwrap();
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Block));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Pipeline));
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
    }

    #[test]
    fn rejects_unterminated_string() {
        let err = tokenize("block A { use x = Format(\"hello) }").unwrap_err();
        assert!(matches!(err, LexError::UnterminatedString { .. }));
    }

    #[test]
    fn rejects_invalid_char() {
        let err = tokenize("block A { @ }").unwrap_err();
        assert!(matches!(err, LexError::UnexpectedChar { ch: '@', .. }));
    }
}
