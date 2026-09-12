#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.end - self.start
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind<'src> {
    Identifier(&'src str),
    KwPublic, KwFn, KwLet, KwIf, KwElse, KwWhile, KwFor, KwIn, KwRange,
    KwEnum, KwSelf, KwEnd,
    KwTrue, KwFalse, KwNone,
    KwInclude,

    IntLiteral(&'src str),
    FloatLiteral(&'src str),
    StringLiteral(&'src str),
    RawStringLiteral(&'src str),
    CharLiteral(&'src str),

    Spaceship,
    HashEqEq,
    DollarTilde,
    FatArrow,
    ThinArrow,
    PipeEq,
    TildeEq,
    ColonEq,
    AtSlash,
    AtEq,
    MinusEq,
    EqEq,
    NotEq,
    LessEq,
    GreaterEq,
    At,
    Question,
    Plus, Minus, Star, Slash, Percent,
    Eq, Less, Greater, Tilde, Pipe, Ampersand, Hash, Dollar,

    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Comma, Semicolon, Colon, Dot,

    Comment(&'src str),
    Whitespace(&'src str),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token<'src> {
    pub kind: TokenKind<'src>,
    pub span: Span,
}

pub struct Lexer<'src> {
    source: &'src str,
    current: usize,
}

impl<'src> Lexer<'src> {
    pub fn new(source: &'src str) -> Self {
        Self { source, current: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.current..].chars().next()
    }

    fn peek_next(&self) -> Option<char> {
        let mut chars = self.source[self.current..].chars();
        chars.next();
        chars.next()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek();
        if let Some(c) = ch {
            self.current += c.len_utf8();
        }
        ch
    }

    #[allow(dead_code)]
    fn match_char(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn slice(&self, start: usize, end: usize) -> &'src str {
        &self.source[start..end]
    }

    fn skip_whitespace(&mut self) -> Option<Token<'src>> {
        let start = self.current;
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
        if self.current > start {
            Some(Token {
                kind: TokenKind::Whitespace(self.slice(start, self.current)),
                 span: Span { start, end: self.current },
            })
        } else {
            None
        }
    }

    fn lex_comment(&mut self) -> Option<Token<'src>> {
        if self.peek() == Some('/') && self.peek_next() == Some('/') {
            let start = self.current;
            self.advance();
            self.advance();
            while let Some(ch) = self.peek() {
                if ch == '\n' {
                    break;
                }
                self.advance();
            }
            return Some(Token {
                kind: TokenKind::Comment(self.slice(start, self.current)),
                        span: Span { start, end: self.current },
            });
        }
        None
    }

    fn lex_identifier_or_keyword(&mut self) -> Token<'src> {
        let start = self.current;
        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || ch == '_' {
                self.advance();
            } else {
                break;
            }
        }
        let text = self.slice(start, self.current);
        let kind = match text {
            "public" => TokenKind::KwPublic,
            "fn" => TokenKind::KwFn,
            "let" => TokenKind::KwLet,
            "if" => TokenKind::KwIf,
            "else" => TokenKind::KwElse,
            "while" => TokenKind::KwWhile,
            "for" => TokenKind::KwFor,
            "in" => TokenKind::KwIn,
            "range" => TokenKind::KwRange,
            "end" => TokenKind::KwEnd,
            "enum" => TokenKind::KwEnum,
            "Self" => TokenKind::KwSelf,
            "True" => TokenKind::KwTrue,
            "False" => TokenKind::KwFalse,
            "None" => TokenKind::KwNone,
            "include" => TokenKind::KwInclude,
            _ => TokenKind::Identifier(text),
        };
        Token {
            kind,
            span: Span { start, end: self.current },
        }
    }

    fn lex_number(&mut self) -> Token<'src> {
        let start = self.current;
        let mut is_float = false;

        // Liczby z prefiksem (0x/0b/0o) nie mają części ułamkowej ani
        // wykładnika - obsługujemy je w osobnej, prostszej pętli. Poprzednia
        // wersja konsumowała tylko prefiks i wracała do pętli dziesiętnej,
        // przez co np. `0x1A` lexerowało się jako `0x` + osobny identyfikator
        // `A` (błąd wykryty przy pisaniu dokumentacji).
        if self.peek() == Some('0') {
            if let Some(next) = self.peek_next() {
                let digit_ok: Option<fn(char) -> bool> = match next {
                    'x' | 'X' => Some(|c: char| c.is_ascii_hexdigit()),
                    'b' | 'B' => Some(|c: char| c == '0' || c == '1'),
                    'o' | 'O' => Some(|c: char| ('0'..='7').contains(&c)),
                    _ => None,
                };
                if let Some(is_digit) = digit_ok {
                    self.advance(); // '0'
                    self.advance(); // x/b/o
                    while let Some(ch) = self.peek() {
                        if ch == '_' || is_digit(ch) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    let text = self.slice(start, self.current);
                    return Token {
                        kind: TokenKind::IntLiteral(text),
                        span: Span { start, end: self.current },
                    };
                }
            }
        }

        while let Some(ch) = self.peek() {
            if ch == '_' || ch.is_ascii_digit() {
                self.advance();
            } else if ch == '.' && !is_float {
                if let Some(next) = self.peek_next() {
                    if next.is_ascii_digit() {
                        is_float = true;
                        self.advance();
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            } else if ch == 'e' || ch == 'E' {
                is_float = true;
                self.advance();
                if self.peek() == Some('+') || self.peek() == Some('-') {
                    self.advance();
                }
            } else {
                break;
            }
        }

        if self.peek() == Some('_') {
            self.advance();
            while let Some(ch) = self.peek() {
                if ch.is_alphanumeric() {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        let text = self.slice(start, self.current);
        let kind = if is_float {
            TokenKind::FloatLiteral(text)
        } else {
            TokenKind::IntLiteral(text)
        };

        Token {
            kind,
            span: Span { start, end: self.current },
        }
    }

    fn lex_string(&mut self) -> Result<Token<'src>, LexError> {
        let start = self.current;
        self.advance();
        let mut escaped = false;

        while let Some(ch) = self.peek() {
            if escaped {
                escaped = false;
                self.advance();
            } else if ch == '\\' {
                escaped = true;
                self.advance();
            } else if ch == '"' {
                self.advance();
                return Ok(Token {
                    kind: TokenKind::StringLiteral(self.slice(start, self.current)),
                          span: Span { start, end: self.current },
                });
            } else {
                self.advance();
            }
        }
        Err(LexError {
            span: Span { start, end: self.current },
            message: "Unterminated string literal".to_string(),
        })
    }

    fn lex_raw_string(&mut self) -> Result<Token<'src>, LexError> {
        let start = self.current;
        self.advance();
        if self.peek() != Some('"') {
            return Err(LexError {
                span: Span { start, end: self.current },
                message: "Expected '\"' after 'r' for raw string".to_string(),
            });
        }
        self.advance();

        while let Some(ch) = self.peek() {
            if ch == '"' {
                self.advance();
                return Ok(Token {
                    kind: TokenKind::RawStringLiteral(self.slice(start, self.current)),
                          span: Span { start, end: self.current },
                });
            } else {
                self.advance();
            }
        }
        Err(LexError {
            span: Span { start, end: self.current },
            message: "Unterminated raw string literal".to_string(),
        })
    }

    fn lex_char(&mut self) -> Result<Token<'src>, LexError> {
        let start = self.current;
        self.advance();
        if let Some(ch) = self.peek() {
            if ch == '\\' {
                self.advance();
                self.advance();
            } else {
                self.advance();
            }
        }
        if self.peek() == Some('\'') {
            self.advance();
            return Ok(Token {
                kind: TokenKind::CharLiteral(self.slice(start, self.current)),
                      span: Span { start, end: self.current },
            });
        }
        Err(LexError {
            span: Span { start, end: self.current },
            message: "Unterminated char literal".to_string(),
        })
    }

    fn lex_operator_or_punct(&mut self) -> Token<'src> {
        let _start = self.current;

        if self.source[self.current..].starts_with("<=>") {
            self.current += 3;
            return Token { kind: TokenKind::Spaceship, span: Span { start: _start, end: self.current } };
        }
        if self.source[self.current..].starts_with("#==") {
            self.current += 3;
            return Token { kind: TokenKind::HashEqEq, span: Span { start: _start, end: self.current } };
        }
        if self.source[self.current..].starts_with("$~") {
            self.current += 2;
            return Token { kind: TokenKind::DollarTilde, span: Span { start: _start, end: self.current } };
        }

        let two_char = if self.current + 1 < self.source.len() {
            Some(&self.source[self.current..self.current+2])
        } else {
            None
        };

        if let Some(op) = two_char {
            match op {
                "=>" => { self.current += 2; return Token { kind: TokenKind::FatArrow, span: Span { start: _start, end: self.current } }; }
                "->" => { self.current += 2; return Token { kind: TokenKind::ThinArrow, span: Span { start: _start, end: self.current } }; }
                "|=" => { self.current += 2; return Token { kind: TokenKind::PipeEq, span: Span { start: _start, end: self.current } }; }
                "~=" => { self.current += 2; return Token { kind: TokenKind::TildeEq, span: Span { start: _start, end: self.current } }; }
                ":=" => { self.current += 2; return Token { kind: TokenKind::ColonEq, span: Span { start: _start, end: self.current } }; }
                "@/" => { self.current += 2; return Token { kind: TokenKind::AtSlash, span: Span { start: _start, end: self.current } }; }
                "@=" => { self.current += 2; return Token { kind: TokenKind::AtEq, span: Span { start: _start, end: self.current } }; }
                "-=" => { self.current += 2; return Token { kind: TokenKind::MinusEq, span: Span { start: _start, end: self.current } }; }
                "==" => { self.current += 2; return Token { kind: TokenKind::EqEq, span: Span { start: _start, end: self.current } }; }
                "!=" => { self.current += 2; return Token { kind: TokenKind::NotEq, span: Span { start: _start, end: self.current } }; }
                "<=" => { self.current += 2; return Token { kind: TokenKind::LessEq, span: Span { start: _start, end: self.current } }; }
                ">=" => { self.current += 2; return Token { kind: TokenKind::GreaterEq, span: Span { start: _start, end: self.current } }; }
                _ => {}
            }
        }

        let ch = self.advance().unwrap();
        let kind = match ch {
            '@' => TokenKind::At,
            '?' => TokenKind::Question,
            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '=' => TokenKind::Eq,
            '<' => TokenKind::Less,
            '>' => TokenKind::Greater,
            '~' => TokenKind::Tilde,
            '|' => TokenKind::Pipe,
            '&' => TokenKind::Ampersand,
            '#' => TokenKind::Hash,
            '$' => TokenKind::Dollar,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ';' => TokenKind::Semicolon,
            ':' => TokenKind::Colon,
            '.' => TokenKind::Dot,
            _ => unreachable!("Should have been caught by is_alphanumeric or whitespace"),
        };

        Token {
            kind,
            span: Span { start: _start, end: self.current },
        }
    }
}

impl<'src> Iterator for Lexer<'src> {
    type Item = Result<Token<'src>, LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(ws) = self.skip_whitespace() {
            let _ = ws;
        }

        if self.current >= self.source.len() {
            return None;
        }

        if let Some(comment) = self.lex_comment() {
            return Some(Ok(comment));
        }

        let ch = self.peek().unwrap();

        if ch.is_alphabetic() || ch == '_' {
            if ch == 'r' && self.peek_next() == Some('"') {
                return Some(self.lex_raw_string());
            }
            return Some(Ok(self.lex_identifier_or_keyword()));
        }

        if ch.is_ascii_digit() {
            return Some(Ok(self.lex_number()));
        }

        if ch == '"' {
            return Some(self.lex_string());
        }
        if ch == '\'' {
            return Some(self.lex_char());
        }

        Some(Ok(self.lex_operator_or_punct()))
    }
}
