//! A reader for the S-expressions of KiCad files (`.kicad_sym`, `.kicad_mod`, ...).
//!
//! Lists, bare atoms (keywords and numbers) and double-quoted strings are supported.
//! Quoted strings may contain escapes as KiCad's lexer reads them: `\"`, `\\`, `\a`, `\b`, `\f`,
//! `\n`, `\r`, `\t`, `\v`, a byte in hex (`\x41`, one or two digits) or octal (`\101`, one to
//! three digits). A backslash before another character is kept (`\q` is `\q`), and `\x` without
//! a hex digit is `x`.

use std::fmt;

/// The escapes in quoted strings, as (escaped character, character it stands for). They are
/// shared with the writer of `cuprite_wire`.
pub const ESCAPES: [(char, char); 5] = [
    ('"', '"'),
    ('\\', '\\'),
    ('n', '\n'),
    ('r', '\r'),
    ('t', '\t'),
];

/// Appends the UTF-8 bytes of `c`.
fn push_char(bytes: &mut Vec<u8>, c: char) {
    bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
}

/// A position in the source text (1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// An S-expression node.
#[derive(Debug, Clone, PartialEq)]
pub enum Sexpr {
    List(Vec<Sexpr>, Pos),
    /// A bare token such as a keyword or a number.
    Atom(String, Pos),
    /// A double-quoted string, unescaped.
    Str(String, Pos),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub pos: Pos,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.pos, self.message)
    }
}

impl std::error::Error for ParseError {}

impl From<ParseError> for crate::Error {
    fn from(e: ParseError) -> Self {
        crate::Error::new(e.to_string())
    }
}

impl Sexpr {
    pub fn pos(&self) -> Pos {
        match self {
            Sexpr::List(_, pos) | Sexpr::Atom(_, pos) | Sexpr::Str(_, pos) => *pos,
        }
    }

    /// The text of an atom or a string.
    pub fn text(&self) -> Option<&str> {
        match self {
            Sexpr::Atom(text, _) | Sexpr::Str(text, _) => Some(text),
            Sexpr::List(..) => None,
        }
    }

    pub fn items(&self) -> &[Sexpr] {
        match self {
            Sexpr::List(items, _) => items,
            _ => &[],
        }
    }

    /// The keyword at the head of a list (`symbol` of `(symbol "R" ...)`).
    pub fn head(&self) -> Option<&str> {
        match self.items().first() {
            Some(Sexpr::Atom(name, _)) => Some(name),
            _ => None,
        }
    }

    /// The items after the head.
    pub fn args(&self) -> &[Sexpr] {
        self.items().get(1..).unwrap_or(&[])
    }

    /// The text of the `index`-th item after the head.
    pub fn arg(&self, index: usize) -> Option<&str> {
        self.args().get(index).and_then(Sexpr::text)
    }

    /// Child lists with the given head.
    pub fn children<'a>(&'a self, head: &'a str) -> impl Iterator<Item = &'a Sexpr> + 'a {
        self.args()
            .iter()
            .filter(move |item| item.head() == Some(head))
    }

    /// The first child list with the given head.
    pub fn child(&self, head: &str) -> Option<&Sexpr> {
        self.args().iter().find(|item| item.head() == Some(head))
    }

    /// Whether the list has a bare atom `name` after the head (e.g. `hide` in `(pin ... hide)`).
    pub fn has_atom(&self, name: &str) -> bool {
        self.args()
            .iter()
            .any(|item| matches!(item, Sexpr::Atom(atom, _) if atom == name))
    }

    /// The value of a `(name yes|no)` flag, or of a bare `name` atom (older files), if present.
    pub fn flag(&self, name: &str) -> Option<bool> {
        if let Some(child) = self.child(name) {
            return Some(child.arg(0).is_none_or(|value| value == "yes"));
        }
        self.has_atom(name).then_some(true)
    }
}

/// Parses a whole file: a single top-level expression, optionally surrounded by whitespace.
pub fn parse(text: &str) -> Result<Sexpr, ParseError> {
    let mut parser = Parser {
        chars: text.chars().peekable(),
        pos: Pos { line: 1, column: 1 },
    };
    parser.skip_whitespace();
    let expr = parser.expr()?;
    parser.skip_whitespace();
    if parser.chars.peek().is_some() {
        return Err(parser.error("unexpected text after the top-level expression"));
    }
    Ok(expr)
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    pos: Pos,
}

impl Parser<'_> {
    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            pos: self.pos,
            message: message.into(),
        }
    }

    /// Reads up to `max` digits in `radix`: their value and how many there are.
    fn digits(&mut self, radix: u32, max: u32) -> (u32, u32) {
        let (mut value, mut count) = (0, 0);
        while count < max
            && let Some(digit) = self.chars.peek().and_then(|c| c.to_digit(radix))
        {
            self.bump();
            value = value * radix + digit;
            count += 1;
        }
        (value, count)
    }

    /// Reads the escape after a backslash into `bytes`.
    fn escape(&mut self, bytes: &mut Vec<u8>) -> Result<(), ParseError> {
        // At the backslash.
        let pos = Pos {
            column: self.pos.column - 1,
            ..self.pos
        };
        let error = |message: String| ParseError { pos, message };
        let c = self
            .bump()
            .ok_or_else(|| self.error("unterminated escape"))?;
        let byte = match c {
            'x' => match self.digits(16, 2) {
                (_, 0) => {
                    bytes.push(b'x');
                    return Ok(());
                }
                (value, _) => value,
            },
            '0'..='7' => {
                let (rest, count) = self.digits(8, 2);
                c.to_digit(8).unwrap_or_default() * 8u32.pow(count) + rest
            }
            'a' => 0x07,
            'b' => 0x08,
            'f' => 0x0c,
            'v' => 0x0b,
            c => {
                match ESCAPES.iter().find(|(escaped, _)| *escaped == c) {
                    Some(&(_, unescaped)) => push_char(bytes, unescaped),
                    None => {
                        bytes.push(b'\\');
                        push_char(bytes, c);
                    }
                }
                return Ok(());
            }
        };
        match u8::try_from(byte) {
            Ok(0) => Err(error("a NUL character in a string".into())),
            Ok(byte) => {
                bytes.push(byte);
                Ok(())
            }
            Err(_) => Err(error(format!("the escape \\{byte:o} is not a byte"))),
        }
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        if c == '\n' {
            self.pos.line += 1;
            self.pos.column = 1;
        } else {
            self.pos.column += 1;
        }
        Some(c)
    }

    fn skip_whitespace(&mut self) {
        while self.chars.peek().is_some_and(|c| c.is_whitespace()) {
            self.bump();
        }
    }

    fn expr(&mut self) -> Result<Sexpr, ParseError> {
        let start = self.pos;
        match self.chars.peek() {
            None => Err(self.error("unexpected end of file")),
            Some('(') => {
                self.bump();
                let mut items = Vec::new();
                loop {
                    self.skip_whitespace();
                    match self.chars.peek() {
                        None => {
                            return Err(ParseError {
                                pos: start,
                                message: "unclosed list".into(),
                            });
                        }
                        Some(')') => {
                            self.bump();
                            return Ok(Sexpr::List(items, start));
                        }
                        Some(_) => items.push(self.expr()?),
                    }
                }
            }
            Some(')') => Err(self.error("unexpected ')'")),
            Some('"') => {
                self.bump();
                // Bytes, since escapes may give the bytes of a UTF-8 sequence one by one.
                let mut bytes = Vec::new();
                loop {
                    match self.bump() {
                        None => {
                            return Err(ParseError {
                                pos: start,
                                message: "unterminated string".into(),
                            });
                        }
                        Some('"') => {
                            return String::from_utf8(bytes)
                                .map(|text| Sexpr::Str(text, start))
                                .map_err(|_| ParseError {
                                    pos: start,
                                    message: "the escapes of the string are not UTF-8".into(),
                                });
                        }
                        Some('\\') => self.escape(&mut bytes)?,
                        Some(c) => push_char(&mut bytes, c),
                    }
                }
            }
            Some(_) => {
                let mut text = String::new();
                while let Some(&c) = self.chars.peek() {
                    if c.is_whitespace() || c == '(' || c == ')' || c == '"' {
                        break;
                    }
                    text.push(c);
                    self.bump();
                }
                Ok(Sexpr::Atom(text, start))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse() {
        let expr = parse("(a (b \"x \\\"y\\\"\\n\" 1.5)\n  (c) hide)").unwrap();
        assert_eq!(expr.head(), Some("a"));
        let b = expr.child("b").unwrap();
        assert_eq!(b.arg(0), Some("x \"y\"\n"));
        assert_eq!(b.arg(1), Some("1.5"));
        assert_eq!(expr.child("c").unwrap().pos(), Pos { line: 2, column: 3 });
        assert!(expr.has_atom("hide"));
        assert_eq!(expr.flag("hide"), Some(true));
        assert_eq!(expr.flag("missing"), None);
    }

    #[test]
    fn test_escapes() {
        // Compared with KiCad 10 (a footprint description read by `pcbnew.FootprintLoad`).
        let cases = [
            (r"a\x41b", "aAb"),
            (r"a\101b", "aAb"),
            (r"a\1234", "aS4"),
            (r"a\x414", "aA4"),
            (r"a\x4G", "a\x04G"),
            (r"a\xGb", "axGb"),
            (r"a\bb\fc\vd\a", "a\x08b\x0cc\x0bd\x07"),
            (r"a\qb", r"a\qb"),
            (r"a\8b", r"a\8b"),
            (r"a\/b", r"a\/b"),
            (r#"a\"b\\c\n"#, "a\"b\\c\n"),
            (r"\xe2\x82\xac", "\u{20ac}"),
        ];
        for (escaped, text) in cases {
            let expr = parse(&format!("(a \"{escaped}\")")).unwrap();
            assert_eq!(expr.arg(0), Some(text), "{escaped}");
        }
    }

    #[test]
    fn test_flags() {
        let expr = parse("(pin (hide yes) (in_bom no) (power))").unwrap();
        assert_eq!(expr.flag("hide"), Some(true));
        assert_eq!(expr.flag("in_bom"), Some(false));
        assert_eq!(expr.flag("power"), Some(true));
    }

    #[test]
    fn test_errors() {
        let error = |text| parse(text).unwrap_err().to_string();
        assert_eq!(error("(a\n (b)"), "1:1: unclosed list");
        assert_eq!(error("(a \"b)"), "1:4: unterminated string");
        assert_eq!(
            error("(a))"),
            "1:4: unexpected text after the top-level expression"
        );
        assert_eq!(error(")"), "1:1: unexpected ')'");
        assert_eq!(error("  "), "1:3: unexpected end of file");
        // KiCad reads such a string as empty (not UTF-8) or cut at the NUL.
        assert_eq!(
            error("(a \"\\xff\")"),
            "1:4: the escapes of the string are not UTF-8"
        );
        assert_eq!(error("(a \"x\\0\")"), "1:6: a NUL character in a string");
        assert_eq!(
            error("(a \"\\777\")"),
            "1:5: the escape \\777 is not a byte"
        );
    }
}
