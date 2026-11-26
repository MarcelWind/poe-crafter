//! Small parser for the generated Lua data tables shipped by Path of Building.
//!
//! The files are data-only: `return { ... }` tables, and `itemBases["X"] = {...}`
//! assignments in `Bases/*.lua`. This parser intentionally supports only that
//! subset, so it can run without a Lua runtime or executing untrusted code.

use std::collections::BTreeMap;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LuaError {
    #[error("lua error at byte {pos}: {message}")]
    Syntax { pos: usize, message: String },
    #[error("expected {expected}, found {found}")]
    Expected {
        expected: &'static str,
        found: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LuaValue {
    Nil,
    Bool(bool),
    Number(f64),
    String(String),
    Table(LuaTable),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LuaTable {
    pub fields: BTreeMap<String, LuaValue>,
    pub numeric_fields: BTreeMap<i64, LuaValue>,
    pub array: Vec<LuaValue>,
}

impl LuaTable {
    pub fn get(&self, key: &str) -> Option<&LuaValue> {
        self.fields.get(key)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(LuaValue::String(value)) => Some(value),
            _ => None,
        }
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        match self.get(key) {
            Some(LuaValue::Number(value)) => Some(*value),
            _ => None,
        }
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get_f64(key).map(|value| value as i64)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.get(key) {
            Some(LuaValue::Bool(value)) => Some(*value),
            _ => None,
        }
    }

    pub fn get_table(&self, key: &str) -> Option<&LuaTable> {
        match self.get(key) {
            Some(LuaValue::Table(value)) => Some(value),
            _ => None,
        }
    }

    pub fn array_strings(&self) -> impl Iterator<Item = &str> {
        self.array.iter().filter_map(|value| match value {
            LuaValue::String(text) => Some(text.as_str()),
            _ => None,
        })
    }

    pub fn array_numbers(&self) -> impl Iterator<Item = f64> + '_ {
        self.array.iter().filter_map(|value| match value {
            LuaValue::Number(number) => Some(*number),
            _ => None,
        })
    }

    pub fn string_list(&self, key: &str) -> Vec<String> {
        self.get_table(key)
            .map(|table| table.array_strings().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    pub fn number_list(&self, key: &str) -> Vec<f64> {
        self.get_table(key)
            .map(|table| table.array_numbers().collect())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.numeric_fields.is_empty() && self.array.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Num(f64),
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Eq,
    Comma,
    Semi,
    Other(char),
    Eof,
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Token::Ident(value) => format!("identifier `{value}`"),
            Token::Str(value) => format!("string `{value}`"),
            Token::Num(value) => format!("number `{value}`"),
            Token::LBrace => "`{`".to_string(),
            Token::RBrace => "`}`".to_string(),
            Token::LBracket => "`[`".to_string(),
            Token::RBracket => "`]`".to_string(),
            Token::Eq => "`=`".to_string(),
            Token::Comma => "`,`".to_string(),
            Token::Semi => "`;`".to_string(),
            Token::Other(value) => format!("`{value}`"),
            Token::Eof => "end of file".to_string(),
        }
    }
}

struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        let bytes = source.as_bytes();
        let pos = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            3
        } else {
            0
        };
        Self { bytes, pos }
    }

    fn error(&self, message: impl Into<String>) -> LuaError {
        LuaError::Syntax {
            pos: self.pos,
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Some(byte)
    }

    fn skip_trivia(&mut self) -> Result<(), LuaError> {
        loop {
            while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                self.pos += 1;
            }
            if self.peek() == Some(b'-') && self.peek_at(1) == Some(b'-') {
                self.pos += 2;
                if self.peek() == Some(b'[') && self.peek_at(1) == Some(b'[') {
                    self.pos += 2;
                    while self.pos + 1 < self.bytes.len() {
                        if self.peek() == Some(b']') && self.peek_at(1) == Some(b']') {
                            self.pos += 2;
                            break;
                        }
                        self.pos += 1;
                    }
                } else {
                    while let Some(byte) = self.peek() {
                        if byte == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                continue;
            }
            break;
        }
        Ok(())
    }

    fn next_token(&mut self) -> Result<Token, LuaError> {
        self.skip_trivia()?;
        let Some(byte) = self.peek() else {
            return Ok(Token::Eof);
        };
        match byte {
            b'{' => {
                self.pos += 1;
                Ok(Token::LBrace)
            }
            b'}' => {
                self.pos += 1;
                Ok(Token::RBrace)
            }
            b'[' => {
                if self.peek_at(1) == Some(b'[') {
                    self.read_long_string()
                } else {
                    self.pos += 1;
                    Ok(Token::LBracket)
                }
            }
            b']' => {
                self.pos += 1;
                Ok(Token::RBracket)
            }
            b'=' => {
                self.pos += 1;
                Ok(Token::Eq)
            }
            b',' => {
                self.pos += 1;
                Ok(Token::Comma)
            }
            b';' => {
                self.pos += 1;
                Ok(Token::Semi)
            }
            b'"' | b'\'' => self.read_string(byte),
            b'-' if self.peek_at(1).is_some_and(|next| next.is_ascii_digit()) => self.read_number(),
            b'0'..=b'9' => self.read_number(),
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.read_ident(),
            _ => {
                let character = byte as char;
                self.pos += 1;
                Ok(Token::Other(character))
            }
        }
    }

    fn read_ident(&mut self) -> Result<Token, LuaError> {
        let start = self.pos;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_alphanumeric() || byte == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.error("identifier is not valid UTF-8"))?;
        Ok(Token::Ident(text.to_string()))
    }

    fn read_number(&mut self) -> Result<Token, LuaError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.error("number is not valid UTF-8"))?;
        let value = text
            .parse::<f64>()
            .map_err(|_| self.error(format!("invalid number `{text}`")))?;
        Ok(Token::Num(value))
    }

    fn read_string(&mut self, quote: u8) -> Result<Token, LuaError> {
        self.pos += 1;
        let mut output = Vec::new();
        loop {
            let Some(byte) = self.bump() else {
                return Err(self.error("unterminated string"));
            };
            if byte == quote {
                break;
            }
            if byte != b'\\' {
                output.push(byte);
                continue;
            }
            let Some(escape) = self.bump() else {
                return Err(self.error("unterminated escape"));
            };
            match escape {
                b'n' => output.push(b'\n'),
                b'r' => output.push(b'\r'),
                b't' => output.push(b'\t'),
                b'a' => output.push(0x07),
                b'b' => output.push(0x08),
                b'f' => output.push(0x0C),
                b'v' => output.push(0x0B),
                b'\\' => output.push(b'\\'),
                b'"' => output.push(b'"'),
                b'\'' => output.push(b'\''),
                b'0'..=b'9' => {
                    let mut value = (escape - b'0') as u32;
                    for _ in 0..2 {
                        match self.peek() {
                            Some(next @ b'0'..=b'9') => {
                                value = value * 10 + (next - b'0') as u32;
                                self.pos += 1;
                            }
                            _ => break,
                        }
                    }
                    output.push(value as u8);
                }
                b'x' => {
                    let mut value = 0u32;
                    for _ in 0..2 {
                        match self.peek() {
                            Some(next) if next.is_ascii_hexdigit() => {
                                value = value * 16 + (next as char).to_digit(16).unwrap();
                                self.pos += 1;
                            }
                            _ => break,
                        }
                    }
                    output.push(value as u8);
                }
                b'\n' => output.push(b'\n'),
                other => output.push(other),
            }
        }
        let text =
            String::from_utf8(output).map_err(|_| self.error("string is not valid UTF-8"))?;
        Ok(Token::Str(text))
    }

    fn read_long_string(&mut self) -> Result<Token, LuaError> {
        self.pos += 2;
        if self.peek() == Some(b'\n') {
            self.pos += 1;
        }
        let start = self.pos;
        while self.pos + 1 < self.bytes.len() {
            if self.peek() == Some(b']') && self.peek_at(1) == Some(b']') {
                let text = std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|_| self.error("long string is not valid UTF-8"))?
                    .to_string();
                self.pos += 2;
                return Ok(Token::Str(text));
            }
            self.pos += 1;
        }
        Err(self.error("unterminated long string"))
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(source: &str) -> Result<Self, LuaError> {
        let mut lexer = Lexer::new(source);
        let mut tokens = Vec::new();
        loop {
            let token = lexer.next_token()?;
            let done = token == Token::Eof;
            tokens.push(token);
            if done {
                break;
            }
        }
        Ok(Self { tokens, pos: 0 })
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn skip_other(&mut self) -> bool {
        if matches!(self.peek(), Token::Other(_)) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_eq(&mut self) -> Result<(), LuaError> {
        if self.peek() == &Token::Eq {
            self.bump();
            Ok(())
        } else {
            Err(LuaError::Expected {
                expected: "`=`",
                found: self.peek().describe(),
            })
        }
    }

    fn parse_value(&mut self) -> Result<LuaValue, LuaError> {
        match self.peek().clone() {
            Token::LBrace => self.parse_table().map(LuaValue::Table),
            Token::Str(value) => {
                self.bump();
                Ok(LuaValue::String(value))
            }
            Token::Num(value) => {
                self.bump();
                Ok(LuaValue::Number(value))
            }
            Token::Ident(identifier) => match identifier.as_str() {
                "true" => {
                    self.bump();
                    Ok(LuaValue::Bool(true))
                }
                "false" => {
                    self.bump();
                    Ok(LuaValue::Bool(false))
                }
                "nil" => {
                    self.bump();
                    Ok(LuaValue::Nil)
                }
                _ => {
                    self.bump();
                    Err(LuaError::Expected {
                        expected: "a value",
                        found: Token::Ident(identifier).describe(),
                    })
                }
            },
            other => {
                self.bump();
                Err(LuaError::Expected {
                    expected: "a value",
                    found: other.describe(),
                })
            }
        }
    }

    fn parse_table(&mut self) -> Result<LuaTable, LuaError> {
        if self.peek() != &Token::LBrace {
            return Err(LuaError::Expected {
                expected: "`{`",
                found: self.peek().describe(),
            });
        }
        self.bump();
        let mut table = LuaTable::default();
        loop {
            if self.peek() == &Token::RBrace {
                self.bump();
                return Ok(table);
            }
            if self.peek() == &Token::Eof {
                return Err(LuaError::Expected {
                    expected: "`}`",
                    found: "end of file".to_string(),
                });
            }

            if self.peek() == &Token::LBracket {
                self.bump();
                let key = self.parse_value()?;
                if self.peek() != &Token::RBracket {
                    return Err(LuaError::Expected {
                        expected: "`]`",
                        found: self.peek().describe(),
                    });
                }
                self.bump();
                self.expect_eq()?;
                let value = self.parse_value()?;
                match key {
                    LuaValue::String(key) => {
                        table.fields.insert(key, value);
                    }
                    LuaValue::Number(key) => {
                        table.numeric_fields.insert(key as i64, value);
                    }
                    LuaValue::Bool(key) => {
                        table.fields.insert(key.to_string(), value);
                    }
                    LuaValue::Nil => {
                        return Err(LuaError::Expected {
                            expected: "a string or number key",
                            found: "nil".to_string(),
                        });
                    }
                    LuaValue::Table(_) => {
                        return Err(LuaError::Expected {
                            expected: "a string or number key",
                            found: "a table".to_string(),
                        });
                    }
                }
            } else if let Token::Ident(name) = self.peek() {
                let name = name.clone();
                if self.tokens.get(self.pos + 1) == Some(&Token::Eq) {
                    self.bump();
                    self.expect_eq()?;
                    let value = self.parse_value()?;
                    table.fields.insert(name, value);
                } else {
                    let value = self.parse_value()?;
                    table.array.push(value);
                }
            } else {
                let value = self.parse_value()?;
                table.array.push(value);
            }

            match self.peek() {
                Token::Comma | Token::Semi => {
                    self.bump();
                }
                Token::RBrace => {}
                other => {
                    return Err(LuaError::Expected {
                        expected: "`,` or `}`",
                        found: other.describe(),
                    });
                }
            }
        }
    }
}

/// Parse a file containing a single `return { ... }` table.
pub fn parse_return_table(source: &str) -> Result<LuaTable, LuaError> {
    let mut parser = Parser::new(source)?;
    if let Token::Ident(name) = parser.peek() {
        if name == "return" {
            parser.bump();
        }
    }
    let value = parser.parse_value()?;
    while parser.skip_other() {}
    if parser.peek() != &Token::Eof {
        return Err(LuaError::Expected {
            expected: "end of file",
            found: parser.peek().describe(),
        });
    }
    match value {
        LuaValue::Table(table) => Ok(table),
        other => Err(LuaError::Expected {
            expected: "a table",
            found: format!("{other:?}"),
        }),
    }
}

/// Parse a `Bases/*.lua` file containing `itemBases["Name"] = { ... }` assignments.
pub fn parse_assignments(
    source: &str,
    target: &str,
) -> Result<BTreeMap<String, LuaTable>, LuaError> {
    let mut parser = Parser::new(source)?;
    let mut result = BTreeMap::new();
    loop {
        match parser.peek().clone() {
            Token::Eof => break,
            Token::Ident(name) if name == target => {
                let saved = parser.pos;
                parser.bump();
                if parser.peek() == &Token::LBracket {
                    parser.bump();
                    let key = match parser.bump() {
                        Token::Str(key) => key,
                        _other => {
                            parser.pos = saved;
                            parser.bump();
                            continue;
                        }
                    };
                    if parser.peek() != &Token::RBracket
                        || parser.tokens.get(parser.pos + 1) != Some(&Token::Eq)
                    {
                        parser.pos = saved;
                        parser.bump();
                        continue;
                    }
                    parser.bump();
                    parser.expect_eq()?;
                    let value = parser.parse_value()?;
                    if let LuaValue::Table(table) = value {
                        result.insert(key, table);
                    }
                }
            }
            _ => {
                parser.bump();
            }
        }
    }
    Ok(result)
}

impl LuaValue {
    pub fn as_table(&self) -> Option<&LuaTable> {
        match self {
            LuaValue::Table(table) => Some(table),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            LuaValue::String(text) => Some(text),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mod_table() {
        let source = r#"
            -- generated
            return {
                ["Strength1"] = { type = "Suffix", affix = "of the Brute", "+(8-12) to Strength",
                    statOrder = { 848 }, level = 1, group = "Strength",
                    weightKey = { "ring", "amulet", "default", },
                    weightVal = { 1000, 1000, 0 },
                    modTags = { "attribute" },
                    tradeHashes = { [4080418644] = { "+(8-12) to Strength" }, }, },
            }
        "#;
        let table = parse_return_table(source).unwrap();
        let mod_table = table.get_table("Strength1").unwrap();
        assert_eq!(mod_table.get_str("type"), Some("Suffix"));
        assert_eq!(mod_table.get_str("affix"), Some("of the Brute"));
        assert_eq!(mod_table.get_i64("level"), Some(1));
        assert_eq!(
            mod_table.array_strings().collect::<Vec<_>>(),
            vec!["+(8-12) to Strength"]
        );
        assert_eq!(
            mod_table.number_list("weightVal"),
            vec![1000.0, 1000.0, 0.0]
        );
        let hashes = mod_table.get_table("tradeHashes").unwrap();
        assert_eq!(
            hashes
                .numeric_fields
                .get(&4080418644)
                .map(|value| match value {
                    LuaValue::Table(table) => table.array_strings().collect::<Vec<_>>(),
                    _ => vec![],
                }),
            Some(vec!["+(8-12) to Strength"])
        );
    }

    #[test]
    fn parses_bases_assignments() {
        let source = r#"
            -- generated
            local itemBases = ...
            itemBases["Coral Ring"] = {
                type = "Ring",
                tags = { ring = true, default = true, },
                implicitModTypes = { },
                req = { level = 1, },
            }
            itemBases["Cord Belt"] = {
                type = "Belt",
                tags = { belt = true, default = true, },
                implicitModTypes = { "Life" },
                req = { level = 1, },
            }
        "#;
        let bases = parse_assignments(source, "itemBases").unwrap();
        assert_eq!(bases.len(), 2);
        assert_eq!(bases["Coral Ring"].get_str("type"), Some("Ring"));
        assert_eq!(
            bases["Coral Ring"]
                .get_table("tags")
                .unwrap()
                .get_bool("ring"),
            Some(true)
        );
        assert_eq!(
            bases["Cord Belt"]
                .get_table("implicitModTypes")
                .unwrap()
                .array_strings()
                .collect::<Vec<_>>(),
            vec!["Life"]
        );
    }

    #[test]
    fn parses_arrays_and_escapes() {
        let source =
            r#"return { { ["id"] = "x", ["text"] = "a \"quote\"\n", }, { ["id"] = "y", }, }"#;
        let table = parse_return_table(source).unwrap();
        assert_eq!(table.array.len(), 2);
        assert_eq!(
            table.array[0]
                .as_table()
                .and_then(|entry| entry.get_str("text")),
            Some("a \"quote\"\n")
        );
    }
}
