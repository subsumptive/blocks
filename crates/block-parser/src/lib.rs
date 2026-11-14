use block_lexer::{Span, Token, TokenKind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportDecl {
    pub name: String,
    pub alias: Option<String>,
    pub path: String,
    pub span: Span,
}

impl ImportDecl {
    pub fn local_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockAst {
    pub imports: Vec<ImportDecl>,
    pub name: String,
    pub name_span: Span,
    pub inputs: Vec<PortDecl>,
    pub outputs: Vec<PortDecl>,
    pub uses: Vec<UseDecl>,
    pub pipeline: Vec<Assignment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortDecl {
    pub name: String,
    pub ty: TypeName,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TypeName {
    Text,
    Integer,
    Float,
    Boolean,
    Bytes,
    TokenSequence,
}

impl std::fmt::Display for TypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeName::Text => write!(f, "Text"),
            TypeName::Integer => write!(f, "Integer"),
            TypeName::Float => write!(f, "Float"),
            TypeName::Boolean => write!(f, "Boolean"),
            TypeName::Bytes => write!(f, "Bytes"),
            TypeName::TokenSequence => write!(f, "TokenSequence"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Literal {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UseDecl {
    pub alias: String,
    pub component: String,
    pub config: Vec<Literal>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    pub target: String,
    pub target_span: Span,
    pub callee: String,
    pub callee_span: Span,
    pub args: Vec<ArgExpr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArgExpr {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("unexpected token {found:?} at {line}:{col}, expected {expected}")]
    UnexpectedToken {
        expected: String,
        found: TokenKind,
        line: usize,
        col: usize,
    },
    #[error("invalid type '{name}' at {line}:{col}; expected one of Text, Integer, Float, Boolean, Bytes, TokenSequence")]
    InvalidType {
        name: String,
        line: usize,
        col: usize,
    },
    #[error("duplicate declaration of '{name}' at {line}:{col}")]
    DuplicateDeclaration {
        name: String,
        line: usize,
        col: usize,
    },
    #[error("missing pipeline block in '{block_name}'")]
    MissingPipeline { block_name: String },
    #[error("duplicate pipeline block in '{block_name}' at {line}:{col}")]
    DuplicatePipeline {
        block_name: String,
        line: usize,
        col: usize,
    },
}

pub fn parse(tokens: &[Token]) -> Result<BlockAst, ParseError> {
    let mut parser = Parser { tokens, pos: 0 };
    parser.parse_block_ast()
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn check(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn expect(&mut self, kind: TokenKind, expected: &str) -> Result<Token, ParseError> {
        if self.peek().kind == kind {
            Ok(self.advance().clone())
        } else {
            let t = self.peek();
            Err(ParseError::UnexpectedToken {
                expected: expected.to_string(),
                found: t.kind.clone(),
                line: t.span.line,
                col: t.span.col,
            })
        }
    }

    fn expect_ident(&mut self, expected: &str) -> Result<(String, Span), ParseError> {
        let t = self.peek().clone();
        match t.kind {
            TokenKind::Ident(s) => {
                self.advance();
                Ok((s, t.span))
            }
            _ => Err(ParseError::UnexpectedToken {
                expected: expected.to_string(),
                found: t.kind,
                line: t.span.line,
                col: t.span.col,
            }),
        }
    }

    fn parse_block_ast(&mut self) -> Result<BlockAst, ParseError> {
        let mut imports = Vec::new();
        let mut seen_imports = HashSet::new();
        while self.check(&TokenKind::Import) {
            let imp = self.parse_import()?;
            if !seen_imports.insert(imp.local_name().to_string()) {
                return Err(ParseError::DuplicateDeclaration {
                    name: imp.local_name().to_string(),
                    line: imp.span.line,
                    col: imp.span.col,
                });
            }
            imports.push(imp);
        }

        self.expect(TokenKind::Block, "'block'")?;
        let (name, name_span) = self.expect_ident("block name")?;
        self.expect(TokenKind::LBrace, "'{'")?;

        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut uses = Vec::new();
        let mut pipeline: Option<Vec<Assignment>> = None;
        let mut seen_decls = HashSet::new();

        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let tok = self.peek().clone();
            match tok.kind {
                TokenKind::Input => {
                    self.advance();
                    let (pname, pspan) = self.expect_ident("input name")?;
                    if !seen_decls.insert(pname.clone()) {
                        return Err(ParseError::DuplicateDeclaration {
                            name: pname,
                            line: pspan.line,
                            col: pspan.col,
                        });
                    }
                    self.expect(TokenKind::Colon, "':'")?;
                    let ty = self.parse_type()?;
                    inputs.push(PortDecl {
                        name: pname,
                        ty,
                        span: pspan,
                    });
                }
                TokenKind::Output => {
                    self.advance();
                    let (pname, pspan) = self.expect_ident("output name")?;
                    if !seen_decls.insert(pname.clone()) {
                        return Err(ParseError::DuplicateDeclaration {
                            name: pname,
                            line: pspan.line,
                            col: pspan.col,
                        });
                    }
                    self.expect(TokenKind::Colon, "':'")?;
                    let ty = self.parse_type()?;
                    outputs.push(PortDecl {
                        name: pname,
                        ty,
                        span: pspan,
                    });
                }
                TokenKind::Use => {
                    self.advance();
                    let (alias, aspan) = self.expect_ident("component alias")?;
                    if !seen_decls.insert(alias.clone()) {
                        return Err(ParseError::DuplicateDeclaration {
                            name: alias,
                            line: aspan.line,
                            col: aspan.col,
                        });
                    }
                    self.expect(TokenKind::Eq, "'='")?;
                    let (component, _) = self.expect_ident("component name")?;
                    self.expect(TokenKind::LParen, "'('")?;
                    let mut config = Vec::new();
                    while !self.check(&TokenKind::RParen) {
                        config.push(self.parse_literal()?);
                        if self.check(&TokenKind::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen, "')'")?;
                    uses.push(UseDecl {
                        alias,
                        component,
                        config,
                        span: aspan,
                    });
                }
                TokenKind::Pipeline => {
                    self.advance();
                    if pipeline.is_some() {
                        return Err(ParseError::DuplicatePipeline {
                            block_name: name,
                            line: tok.span.line,
                            col: tok.span.col,
                        });
                    }
                    self.expect(TokenKind::LBrace, "'{'")?;
                    let mut stmts = Vec::new();
                    while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
                        stmts.push(self.parse_assignment()?);
                    }
                    self.expect(TokenKind::RBrace, "'}'")?;
                    pipeline = Some(stmts);
                }
                _ => {
                    return Err(ParseError::UnexpectedToken {
                        expected: "'input', 'output', 'use', or 'pipeline'".to_string(),
                        found: tok.kind,
                        line: tok.span.line,
                        col: tok.span.col,
                    });
                }
            }
        }

        self.expect(TokenKind::RBrace, "'}'")?;
        self.expect(TokenKind::Eof, "end of file")?;

        let pipeline = pipeline.ok_or_else(|| ParseError::MissingPipeline {
            block_name: name.clone(),
        })?;

        Ok(BlockAst {
            imports,
            name,
            name_span,
            inputs,
            outputs,
            uses,
            pipeline,
        })
    }

    fn parse_import(&mut self) -> Result<ImportDecl, ParseError> {
        let start = self.expect(TokenKind::Import, "'import'")?.span;
        let (name, _) = self.expect_ident("imported block name")?;
        let alias = if self.check(&TokenKind::As) {
            self.advance();
            let (a, _) = self.expect_ident("import alias")?;
            Some(a)
        } else {
            None
        };
        self.expect(TokenKind::From, "'from'")?;
        let tok = self.peek().clone();
        let path = match tok.kind {
            TokenKind::String(s) => {
                self.advance();
                s
            }
            _ => {
                return Err(ParseError::UnexpectedToken {
                    expected: "import path string".to_string(),
                    found: tok.kind,
                    line: tok.span.line,
                    col: tok.span.col,
                })
            }
        };
        Ok(ImportDecl {
            name,
            alias,
            path,
            span: start,
        })
    }

    fn parse_type(&mut self) -> Result<TypeName, ParseError> {
        let (name, span) = self.expect_ident("type name")?;
        match name.as_str() {
            "Text" => Ok(TypeName::Text),
            "Integer" => Ok(TypeName::Integer),
            "Float" => Ok(TypeName::Float),
            "Boolean" => Ok(TypeName::Boolean),
            "Bytes" => Ok(TypeName::Bytes),
            "TokenSequence" => Ok(TypeName::TokenSequence),
            _ => Err(ParseError::InvalidType {
                name,
                line: span.line,
                col: span.col,
            }),
        }
    }

    fn parse_literal(&mut self) -> Result<Literal, ParseError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::String(s) => {
                self.advance();
                Ok(Literal::String(s))
            }
            TokenKind::Integer(n) => {
                self.advance();
                Ok(Literal::Integer(n))
            }
            TokenKind::Float(f) => {
                self.advance();
                Ok(Literal::Float(f))
            }
            TokenKind::True => {
                self.advance();
                Ok(Literal::Boolean(true))
            }
            TokenKind::False => {
                self.advance();
                Ok(Literal::Boolean(false))
            }
            _ => Err(ParseError::UnexpectedToken {
                expected: "literal configuration argument".to_string(),
                found: tok.kind,
                line: tok.span.line,
                col: tok.span.col,
            }),
        }
    }

    fn parse_assignment(&mut self) -> Result<Assignment, ParseError> {
        let (target, target_span) = self.expect_ident("target variable")?;
        self.expect(TokenKind::Eq, "'='")?;
        let (callee, callee_span) = self.expect_ident("component alias")?;
        self.expect(TokenKind::LParen, "'('")?;
        let mut args = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let (arg_name, arg_span) = self.expect_ident("argument identifier")?;
            args.push(ArgExpr {
                name: arg_name,
                span: arg_span,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RParen, "')'")?;
        Ok(Assignment {
            target,
            target_span,
            callee,
            callee_span,
            args,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use block_lexer::tokenize;

    #[test]
    fn parses_greeting_block() {
        let src = r#"
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
        let ast = parse(&tokens).unwrap();
        assert_eq!(ast.name, "Greeting");
        assert_eq!(ast.inputs.len(), 1);
        assert_eq!(ast.outputs.len(), 1);
        assert_eq!(ast.uses.len(), 1);
        assert_eq!(ast.pipeline.len(), 1);
    }

    #[test]
    fn rejects_missing_block_name() {
        let tokens = tokenize("block { pipeline {} }").unwrap();
        assert!(matches!(
            parse(&tokens),
            Err(ParseError::UnexpectedToken { .. })
        ));
    }

    #[test]
    fn rejects_missing_braces() {
        let tokens = tokenize("block A input x: Text").unwrap();
        assert!(matches!(
            parse(&tokens),
            Err(ParseError::UnexpectedToken { .. })
        ));
    }

    #[test]
    fn rejects_unknown_declaration() {
        let tokens = tokenize("block A { foo bar pipeline {} }").unwrap();
        assert!(matches!(
            parse(&tokens),
            Err(ParseError::UnexpectedToken { .. })
        ));
    }

    #[test]
    fn rejects_invalid_type() {
        let tokens = tokenize("block A { input x: Any pipeline {} }").unwrap();
        assert!(matches!(
            parse(&tokens),
            Err(ParseError::InvalidType { .. })
        ));
    }

    #[test]
    fn rejects_malformed_invocation() {
        let tokens = tokenize("block A { use f = Format(123 pipeline {} }").unwrap();
        assert!(matches!(
            parse(&tokens),
            Err(ParseError::UnexpectedToken { .. })
        ));
    }

    #[test]
    fn rejects_missing_pipeline() {
        let tokens = tokenize("block A { input x: Text }").unwrap();
        assert!(matches!(
            parse(&tokens),
            Err(ParseError::MissingPipeline { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_declarations() {
        let tokens = tokenize("block A { input x: Text input x: Text pipeline {} }").unwrap();
        assert!(matches!(
            parse(&tokens),
            Err(ParseError::DuplicateDeclaration { .. })
        ));
    }
}
