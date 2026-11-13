//! Bounded reader of the TOML subset that `Cargo.lock`, `Cargo.toml` and
//! `rust-toolchain.toml` use, without a TOML dependency.
//!
//! Grammar of format 1: LF or CRLF lines; `#` comments; headers `[a.b]` and
//! `[[a]]` of bare keys `[A-Za-z0-9_-]+` joined by `.`; pairs `bare_key =
//! value`; values: basic strings with the escapes `\"`, `\\`, `\n`, `\t` and
//! `\uXXXX`, literal strings, decimal integers, `true`/`false`, arrays (on
//! several lines, trailing comma and comments allowed) and inline tables on
//! one line. Everything else is refused with its line: quoted or dotted keys,
//! multi-line strings, floats, dates, other escapes, a repeated key in one
//! table, a repeated table header, nesting deeper than 16 and more than
//! 1 000 000 items.
//!
//! The reader decides nothing about the meaning of a file: each caller says
//! whether a refusal refuses the file (`Cargo.lock`) or leaves the facts it
//! would have given unknown (`Cargo.toml`, `rust-toolchain.toml`).
use super::MAX_DECLARATION_BYTES;
use std::collections::BTreeSet;

const MAX_NESTING: usize = 16;
const MAX_ITEMS: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Value {
    String(String),
    Integer(i64),
    Bool(bool),
    Array(Vec<Value>),
    /// An inline table, in the order of its keys in the file.
    Table(Vec<(String, Value)>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Item {
    Header {
        array: bool,
        path: Vec<String>,
        line: usize,
    },
    Pair {
        key: String,
        value: Value,
        line: usize,
    },
}

/// A construct outside the grammar, with the 1-based line it starts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Refused {
    pub(super) line: usize,
    pub(super) reason: &'static str,
}

/// The items of `bytes`, in file order, or the first construct outside the
/// grammar.
pub(super) fn parse(bytes: &[u8]) -> Result<Vec<Item>, Refused> {
    if bytes.len() as u64 > MAX_DECLARATION_BYTES {
        return Err(Refused {
            line: 1,
            reason: "the file exceeds the bound of a declaration file",
        });
    }
    if let Err(error) = std::str::from_utf8(bytes) {
        let line = 1 + bytes[..error.valid_up_to()]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        return Err(Refused {
            line,
            reason: "the file is not UTF-8",
        });
    }
    Parser {
        bytes,
        position: 0,
        line: 1,
        items: 0,
    }
    .document()
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
    line: usize,
    items: usize,
}

impl Parser<'_> {
    fn refuse<T>(&self, reason: &'static str) -> Result<T, Refused> {
        Err(Refused {
            line: self.line,
            reason,
        })
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.position + offset).copied()
    }

    fn starts_with(&self, text: &[u8]) -> bool {
        self.bytes[self.position..].starts_with(text)
    }

    fn skip_blanks(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.position += 1;
        }
    }

    fn count(&mut self) -> Result<(), Refused> {
        self.items += 1;
        if self.items > MAX_ITEMS {
            return self.refuse("the file holds more items than the reader admits");
        }
        Ok(())
    }

    fn document(mut self) -> Result<Vec<Item>, Refused> {
        let mut items = Vec::new();
        let mut keys: BTreeSet<String> = BTreeSet::new();
        let mut tables: BTreeSet<Vec<String>> = BTreeSet::new();
        loop {
            self.skip_blanks();
            match self.peek() {
                None => return Ok(items),
                Some(b'\n' | b'\r' | b'#') => self.end_of_line()?,
                Some(b'[') => {
                    let line = self.line;
                    let (array, path) = self.header()?;
                    self.count()?;
                    if !array && !tables.insert(path.clone()) {
                        return Err(Refused {
                            line,
                            reason: "a table header is repeated",
                        });
                    }
                    keys.clear();
                    items.push(Item::Header { array, path, line });
                    self.end_of_line()?;
                }
                Some(_) => {
                    let line = self.line;
                    let (key, value) = self.pair(0)?;
                    if !keys.insert(key.clone()) {
                        return Err(Refused {
                            line,
                            reason: "a key is repeated in one table",
                        });
                    }
                    items.push(Item::Pair { key, value, line });
                    self.end_of_line()?;
                }
            }
        }
    }

    /// Blanks, an optional comment, then the end of the line or of the input.
    fn end_of_line(&mut self) -> Result<(), Refused> {
        self.skip_blanks();
        if self.peek() == Some(b'#') {
            self.comment()?;
        }
        match self.peek() {
            None => Ok(()),
            Some(b'\n') => {
                self.position += 1;
                self.line += 1;
                Ok(())
            }
            Some(b'\r') if self.peek_at(1) == Some(b'\n') => {
                self.position += 2;
                self.line += 1;
                Ok(())
            }
            Some(_) => self.refuse("unexpected text after an item"),
        }
    }

    /// From `#` to the end of the line, which is not consumed.
    fn comment(&mut self) -> Result<(), Refused> {
        while let Some(byte) = self.peek() {
            if byte == b'\n' || byte == b'\r' {
                break;
            }
            if (byte < 0x20 && byte != b'\t') || byte == 0x7f {
                return self.refuse("a comment holds a control character");
            }
            self.position += 1;
        }
        Ok(())
    }

    /// Blanks, line ends and comments between the values of an array.
    fn skip_layout(&mut self) -> Result<(), Refused> {
        loop {
            self.skip_blanks();
            match self.peek() {
                Some(b'#') => self.comment()?,
                Some(b'\n') => {
                    self.position += 1;
                    self.line += 1;
                }
                Some(b'\r') if self.peek_at(1) == Some(b'\n') => {
                    self.position += 2;
                    self.line += 1;
                }
                _ => return Ok(()),
            }
        }
    }

    fn header(&mut self) -> Result<(bool, Vec<String>), Refused> {
        self.position += 1;
        let array = self.peek() == Some(b'[');
        if array {
            self.position += 1;
        }
        let mut path = Vec::new();
        loop {
            self.skip_blanks();
            path.push(self.bare_key()?);
            if path.len() > MAX_NESTING {
                return self.refuse("a table path is deeper than the reader admits");
            }
            self.skip_blanks();
            match self.peek() {
                Some(b'.') => self.position += 1,
                Some(b']') => break,
                _ => return self.refuse("a table header is outside the supported form"),
            }
        }
        self.position += 1;
        if array {
            if self.peek() != Some(b']') {
                return self.refuse("a table header is outside the supported form");
            }
            self.position += 1;
        }
        Ok((array, path))
    }

    fn bare_key(&mut self) -> Result<String, Refused> {
        if matches!(self.peek(), Some(b'"' | b'\'')) {
            return self.refuse("quoted keys are not supported");
        }
        let start = self.position;
        while matches!(
            self.peek(),
            Some(byte) if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
        ) {
            self.position += 1;
        }
        if self.position == start {
            return self.refuse("a key is missing");
        }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.position]).into_owned())
    }

    fn pair(&mut self, depth: usize) -> Result<(String, Value), Refused> {
        let key = self.bare_key()?;
        self.skip_blanks();
        match self.peek() {
            Some(b'=') => self.position += 1,
            Some(b'.') => return self.refuse("dotted keys are not supported"),
            _ => return self.refuse("a key must be followed by ="),
        }
        self.skip_blanks();
        let value = self.value(depth)?;
        Ok((key, value))
    }

    fn value(&mut self, depth: usize) -> Result<Value, Refused> {
        if depth > MAX_NESTING {
            return self.refuse("values are nested deeper than the reader admits");
        }
        self.count()?;
        match self.peek() {
            Some(b'"') => {
                if self.starts_with(b"\"\"\"") {
                    return self.refuse("multi-line strings are not supported");
                }
                self.basic_string().map(Value::String)
            }
            Some(b'\'') => {
                if self.starts_with(b"'''") {
                    return self.refuse("multi-line strings are not supported");
                }
                self.literal_string().map(Value::String)
            }
            Some(b'[') => self.array(depth + 1),
            Some(b'{') => self.inline_table(depth + 1),
            Some(b't') if self.starts_with(b"true") => {
                self.position += 4;
                self.after_value()?;
                Ok(Value::Bool(true))
            }
            Some(b'f') if self.starts_with(b"false") => {
                self.position += 5;
                self.after_value()?;
                Ok(Value::Bool(false))
            }
            Some(b'0'..=b'9' | b'+' | b'-') => self.integer(),
            _ => self.refuse("a value is outside the supported forms"),
        }
    }

    /// What may follow a complete value on its line.
    fn after_value(&self) -> Result<(), Refused> {
        match self.peek() {
            None | Some(b' ' | b'\t' | b'\n' | b'\r' | b',' | b']' | b'}' | b'#') => Ok(()),
            Some(_) => self.refuse("unexpected character after a value"),
        }
    }

    fn integer(&mut self) -> Result<Value, Refused> {
        let start = self.position;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.position += 1;
        }
        let digits = self.position;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.position += 1;
        }
        if self.position == digits {
            return self.refuse("a value is outside the supported forms");
        }
        match self.peek() {
            Some(b'.' | b'e' | b'E') => return self.refuse("floats are not supported"),
            Some(b'-' | b':') => return self.refuse("dates and times are not supported"),
            Some(b'_') => return self.refuse("integers with underscores are not supported"),
            Some(b'x' | b'o' | b'b') => {
                return self.refuse("non-decimal integers are not supported");
            }
            _ => {}
        }
        self.after_value()?;
        let text = String::from_utf8_lossy(&self.bytes[start..self.position]);
        match text.parse::<i64>() {
            Ok(number) => Ok(Value::Integer(number)),
            Err(_) => self.refuse("an integer is out of range"),
        }
    }

    fn basic_string(&mut self) -> Result<String, Refused> {
        self.position += 1;
        let mut text: Vec<u8> = Vec::new();
        loop {
            let Some(byte) = self.peek() else {
                return self.refuse("a string is not terminated");
            };
            match byte {
                b'"' => {
                    self.position += 1;
                    break;
                }
                b'\\' => {
                    let escape = self.peek_at(1);
                    self.position += 2;
                    match escape {
                        Some(b'"') => text.push(b'"'),
                        Some(b'\\') => text.push(b'\\'),
                        Some(b'n') => text.push(b'\n'),
                        Some(b't') => text.push(b'\t'),
                        Some(b'u') => {
                            let character = self.unicode_escape()?;
                            let mut buffer = [0_u8; 4];
                            text.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                        }
                        _ => return self.refuse("an escape sequence is not supported"),
                    }
                }
                b'\n' | b'\r' => return self.refuse("a string is not terminated"),
                byte if (byte < 0x20 && byte != b'\t') || byte == 0x7f => {
                    return self.refuse("a string holds a control character");
                }
                byte => {
                    text.push(byte);
                    self.position += 1;
                }
            }
        }
        self.after_value()?;
        match String::from_utf8(text) {
            Ok(text) => Ok(text),
            Err(_) => self.refuse("a string is not UTF-8"),
        }
    }

    /// The four hexadecimal digits after `\u`.
    fn unicode_escape(&mut self) -> Result<char, Refused> {
        let Some(digits) = self.bytes.get(self.position..self.position + 4) else {
            return self.refuse("an escape sequence is not supported");
        };
        if !digits.iter().all(|digit| digit.is_ascii_hexdigit()) {
            return self.refuse("an escape sequence is not supported");
        }
        let code = u32::from_str_radix(&String::from_utf8_lossy(digits), 16);
        let Some(character) = code.ok().and_then(char::from_u32) else {
            return self.refuse("an escape sequence names no character");
        };
        self.position += 4;
        Ok(character)
    }

    fn literal_string(&mut self) -> Result<String, Refused> {
        self.position += 1;
        let start = self.position;
        loop {
            match self.peek() {
                None | Some(b'\n' | b'\r') => return self.refuse("a string is not terminated"),
                Some(b'\'') => break,
                Some(byte) if (byte < 0x20 && byte != b'\t') || byte == 0x7f => {
                    return self.refuse("a string holds a control character");
                }
                Some(_) => self.position += 1,
            }
        }
        let text = String::from_utf8_lossy(&self.bytes[start..self.position]).into_owned();
        self.position += 1;
        self.after_value()?;
        Ok(text)
    }

    fn array(&mut self, depth: usize) -> Result<Value, Refused> {
        if depth > MAX_NESTING {
            return self.refuse("values are nested deeper than the reader admits");
        }
        self.position += 1;
        let mut values = Vec::new();
        loop {
            self.skip_layout()?;
            if self.peek() == Some(b']') {
                self.position += 1;
                break;
            }
            values.push(self.value(depth)?);
            self.skip_layout()?;
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    break;
                }
                _ => return self.refuse("an array is not terminated"),
            }
        }
        self.after_value()?;
        Ok(Value::Array(values))
    }

    fn inline_table(&mut self, depth: usize) -> Result<Value, Refused> {
        if depth > MAX_NESTING {
            return self.refuse("values are nested deeper than the reader admits");
        }
        self.position += 1;
        let mut entries: Vec<(String, Value)> = Vec::new();
        self.skip_blanks();
        if self.peek() == Some(b'}') {
            self.position += 1;
            self.after_value()?;
            return Ok(Value::Table(entries));
        }
        loop {
            self.skip_blanks();
            if matches!(self.peek(), Some(b'\n' | b'\r')) {
                return self.refuse("an inline table must stay on one line");
            }
            let (key, value) = self.pair(depth)?;
            if entries.iter().any(|(known, _)| *known == key) {
                return self.refuse("a key is repeated in one table");
            }
            entries.push((key, value));
            self.skip_blanks();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    break;
                }
                Some(b'\n' | b'\r') => {
                    return self.refuse("an inline table must stay on one line");
                }
                _ => return self.refuse("an inline table is not terminated"),
            }
        }
        self.after_value()?;
        Ok(Value::Table(entries))
    }
}
