//! Definitions in SPICE model libraries (`.lib`, `.sp`, `.sub`, ...).
//!
//! Only what is needed to use a model from a part is read: the headers of the top-level
//! `.subckt` definitions (ports and parameters), the types of the top-level `.model` statements,
//! the names of the top-level `.param` / `.func` definitions, the files they include, and hints of
//! the dialect the library is written in. The bodies of the definitions are kept as text, to be
//! copied into a deck, but not interpreted: ngspice reads them.
//!
//! Vendor libraries are often written for PSpice: `.LIB "file"` includes a file, `$` is not a
//! comment (`$G_DPWR` is a node name), and `OPTIONAL:` ports and encrypted blocks
//! (`$CDNENCSTART` ... `$CDNENCFINISH`) may appear. These are recognized; see
//! [Parts from SPICE models](https://github.com/hyranno/cuprite_wire/blob/main/doc/spice_part_macro.md) for what ngspice can simulate.

use crate::Error;

/// The SPICE dialect a library is written in, which selects the compatibility mode of ngspice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dialect {
    Ngspice,
    Pspice,
    Ltspice,
}

impl Dialect {
    pub fn as_str(self) -> &'static str {
        match self {
            Dialect::Ngspice => "ngspice",
            Dialect::Pspice => "pspice",
            Dialect::Ltspice => "ltspice",
        }
    }
}

/// A top-level definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub name: String,
    pub kind: DefinitionKind,
    /// The definition as written in the library, from the `.subckt` / `.model` / `.func` line to
    /// `.ends` / the last continuation line, ending with a newline. A `.param` statement defining
    /// several parameters is split into one `.param name=value` line per parameter.
    pub text: String,
    /// The `.lib` section the definition is in (`.lib name` ... `.endl`).
    pub section: Option<String>,
    /// The definition contains an encrypted block, which ngspice cannot read.
    pub encrypted: bool,
    /// The line where the definition starts (1-based).
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionKind {
    /// `.subckt NAME port... [OPTIONAL: port=default...] [PARAMS:] [name=value...]`
    Subckt {
        ports: Vec<String>,
        /// PSpice's `OPTIONAL:` ports with their default nodes.
        optional_ports: Vec<(String, String)>,
        /// Parameters with their default values, as written (`PARAMS:` / `TEXT:` or bare `name=value`).
        params: Vec<(String, String)>,
    },
    /// `.model NAME TYPE ...`; the type is lowercased (`d`, `npn`, `vdmos`, ...).
    Model { model_type: String },
    /// `.param NAME=value`
    Param,
    /// `.func NAME(args) ...`
    Func,
}

/// A file read by a top-level `.include` / `.inc` / `.lib` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Include {
    /// The file, as written.
    pub path: String,
    /// `.lib file section`: only that section of the file is read.
    pub section: Option<String>,
    /// The section of this library the line is in.
    pub within: Option<String>,
    /// `.lib file` without a section: PSpice's include, which ngspice reads only in its PSpice
    /// compatibility mode.
    pub pspice: bool,
}

/// A hint of the dialect: a construct that only that dialect (of those ngspice emulates) reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialectHint {
    pub dialect: Dialect,
    /// What was found, e.g. `IF(`.
    pub what: &'static str,
    pub line: usize,
}

impl std::fmt::Display for DialectHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}` (line {})", self.what, self.line)
    }
}

/// The definitions of a library file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpiceLib {
    pub definitions: Vec<Definition>,
    pub includes: Vec<Include>,
    /// Names of the `.lib` sections.
    pub sections: Vec<String>,
    /// The first occurrence of each construct hinting at a dialect.
    pub hints: Vec<DialectHint>,
    /// Encrypted blocks outside any definition (they may hide whole definitions).
    pub encrypted_blocks: usize,
}

/// A statement: a line with its continuation lines.
struct Statement<'a> {
    /// The text without comments, continuation lines joined with a space.
    text: String,
    /// The physical lines, including comment lines that follow.
    lines: Vec<&'a str>,
    line: usize,
    /// An encrypted block (`$CDNENCSTART` ... `$CDNENCFINISH`), as one statement.
    encrypted: bool,
}

impl Statement<'_> {
    /// The physical lines without the trailing comment lines (which belong to the next statement).
    fn own_lines(&self) -> &[&str] {
        let end = self
            .lines
            .iter()
            .rposition(|l| !is_comment_line(l))
            .map_or(1, |i| i + 1);
        &self.lines[..end]
    }
}

struct OpenSubckt<'a> {
    name: String,
    /// `None` for nested subcircuits, which are part of the outer one.
    top: Option<(DefinitionKind, Vec<&'a str>, bool, usize)>,
}

impl SpiceLib {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let statements = statements(text);
        // A single-argument `.lib` starts a section if the file ends sections with `.endl`, and
        // includes a file (PSpice) otherwise.
        let has_sections = statements
            .iter()
            .any(|s| keyword(&s.text).eq_ignore_ascii_case(".endl"));

        let mut lib = SpiceLib::default();
        let mut open: Vec<OpenSubckt> = Vec::new();
        let mut section: Option<String> = None;
        for statement in &statements {
            let at = |message: String| Error::new(format!("line {}: {message}", statement.line));
            if statement.encrypted {
                match open.first_mut().and_then(|s| s.top.as_mut()) {
                    Some((_, lines, encrypted, _)) => {
                        lines.extend(statement.own_lines());
                        *encrypted = true;
                    }
                    None => lib.encrypted_blocks += 1,
                }
                continue;
            }
            lib.collect_hints(statement);
            let tokens = tokenize(&statement.text);
            let keyword = tokens
                .first()
                .map(|t| t.to_ascii_lowercase())
                .unwrap_or_default();
            if let Some(OpenSubckt {
                top: Some((_, lines, _, _)),
                ..
            }) = open.first_mut()
            {
                lines.extend(statement.own_lines());
            }
            match keyword.as_str() {
                ".subckt" => {
                    let name = tokens
                        .get(1)
                        .cloned()
                        .ok_or_else(|| at("`.subckt` without a name".into()))?;
                    let top = open.is_empty().then(|| {
                        (
                            subckt_header(&tokens[2..]),
                            statement.own_lines().to_vec(),
                            false,
                            statement.line,
                        )
                    });
                    open.push(OpenSubckt { name, top });
                }
                ".ends" => {
                    let subckt = open
                        .pop()
                        .ok_or_else(|| at("`.ends` without `.subckt`".into()))?;
                    if let Some(name) = tokens.get(1)
                        && !name.eq_ignore_ascii_case(&subckt.name)
                    {
                        return Err(at(format!(
                            "`.ends {name}` closes `.subckt {}`",
                            subckt.name
                        )));
                    }
                    if let Some((kind, lines, encrypted, line)) = subckt.top {
                        lib.definitions.push(Definition {
                            name: subckt.name,
                            kind,
                            text: join(&lines),
                            section: section.clone(),
                            encrypted,
                            line,
                        });
                    }
                }
                _ if !open.is_empty() => {}
                ".model" => {
                    let name = tokens
                        .get(1)
                        .cloned()
                        .ok_or_else(|| at("`.model` without a name".into()))?;
                    let model_type = tokens
                        .get(2)
                        .map(|t| t.split('(').next().unwrap_or_default().to_ascii_lowercase())
                        .unwrap_or_default();
                    lib.push(
                        statement,
                        section.clone(),
                        name,
                        DefinitionKind::Model { model_type },
                    );
                }
                ".param" => {
                    for token in &tokens[1..] {
                        if let Some((name, _)) = token.split_once('=') {
                            lib.definitions.push(Definition {
                                name: name.to_string(),
                                kind: DefinitionKind::Param,
                                text: format!(".param {token}\n"),
                                section: section.clone(),
                                encrypted: false,
                                line: statement.line,
                            });
                        }
                    }
                }
                ".func" => {
                    let name = tokens
                        .get(1)
                        .and_then(|t| t.split('(').next())
                        .filter(|n| !n.is_empty())
                        .ok_or_else(|| at("`.func` without a name".into()))?
                        .to_string();
                    lib.push(statement, section.clone(), name, DefinitionKind::Func);
                }
                ".include" | ".inc" => {
                    let path = tokens
                        .get(1)
                        .ok_or_else(|| at(format!("`{keyword}` without a file")))?;
                    lib.includes.push(Include {
                        path: unquote(path).to_string(),
                        section: None,
                        within: section.clone(),
                        pspice: false,
                    });
                }
                ".lib" => match &tokens[1..] {
                    [name] if has_sections && section.is_none() => {
                        section = Some(name.clone());
                        lib.sections.push(name.clone());
                    }
                    [path] => {
                        lib.includes.push(Include {
                            path: unquote(path).to_string(),
                            section: None,
                            within: section.clone(),
                            pspice: true,
                        });
                        lib.hint(Dialect::Pspice, ".LIB file", statement.line);
                    }
                    [path, name] => lib.includes.push(Include {
                        path: unquote(path).to_string(),
                        section: Some(name.clone()),
                        within: section.clone(),
                        pspice: false,
                    }),
                    _ => return Err(at("expected `.lib file [section]` or `.lib section`".into())),
                },
                ".endl" if section.take().is_none() => {
                    return Err(at("`.endl` outside a section".into()));
                }
                _ => {}
            }
        }
        if let Some(subckt) = open.first() {
            return Err(Error::new(format!(
                "`.subckt {}` is not closed with `.ends`",
                subckt.name
            )));
        }
        Ok(lib)
    }

    fn push(
        &mut self,
        statement: &Statement,
        section: Option<String>,
        name: String,
        kind: DefinitionKind,
    ) {
        self.definitions.push(Definition {
            name,
            kind,
            text: join(statement.own_lines()),
            section,
            encrypted: false,
            line: statement.line,
        });
    }

    fn hint(&mut self, dialect: Dialect, what: &'static str, line: usize) {
        if !self.hints.iter().any(|hint| hint.what == what) {
            self.hints.push(DialectHint {
                dialect,
                what,
                line,
            });
        }
    }

    /// Constructs that ngspice reads only in a compatibility mode (checked with ngspice 44.2; the
    /// PSpice functions are translated only inside `.subckt`, where vendor models put them).
    fn collect_hints(&mut self, statement: &Statement) {
        let text = statement.text.to_ascii_lowercase();
        // Without the PSpice mode, `IF` and `STP` are unknown and `LIMIT` silently evaluates to 0.
        for (function, what) in [("if", "IF("), ("stp", "STP("), ("limit", "LIMIT(")] {
            if calls(&text, function) {
                self.hint(Dialect::Pspice, what, statement.line);
            }
        }
        for (op, what) in [('&', "&"), ('|', "|")] {
            if single(&text, op) {
                self.hint(Dialect::Pspice, what, statement.line);
            }
        }
        for (function, what) in [("uplim", "uplim("), ("dnlim", "dnlim(")] {
            if calls(&text, function) {
                self.hint(Dialect::Ltspice, what, statement.line);
            }
        }
    }

    /// The `.subckt` or `.model` named `name` outside any section (case-insensitively, like SPICE).
    pub fn find(&self, name: &str) -> Option<&Definition> {
        self.find_in(name, None)
    }

    /// The `.subckt` or `.model` named `name` in `section` (`None`: outside any section).
    pub fn find_in(&self, name: &str, section: Option<&str>) -> Option<&Definition> {
        self.definitions(section).find(|d| {
            matches!(
                d.kind,
                DefinitionKind::Subckt { .. } | DefinitionKind::Model { .. }
            ) && d.name.eq_ignore_ascii_case(name)
        })
    }

    /// The definitions in `section` (`None`: outside any section).
    pub fn definitions(&self, section: Option<&str>) -> impl Iterator<Item = &Definition> {
        self.definitions
            .iter()
            .filter(move |d| same_section(d.section.as_deref(), section))
    }

    /// The includes in `section` (`None`: outside any section).
    pub fn includes(&self, section: Option<&str>) -> impl Iterator<Item = &Include> {
        self.includes
            .iter()
            .filter(move |i| same_section(i.within.as_deref(), section))
    }
}

fn same_section(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

/// Groups the lines into statements.
fn statements(text: &str) -> Vec<Statement<'_>> {
    let mut statements: Vec<Statement> = Vec::new();
    let mut encrypted = false;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        let upper = trimmed.to_ascii_uppercase();
        if encrypted || upper.starts_with("$CDNENCSTART") {
            match statements.last_mut() {
                Some(last) if encrypted => last.lines.push(line),
                _ => statements.push(Statement {
                    text: String::new(),
                    lines: vec![line],
                    line: index + 1,
                    encrypted: true,
                }),
            }
            encrypted = !upper.starts_with("$CDNENCFINISH");
            continue;
        }
        let content = strip_comment(line).trim();
        if is_comment_line(line) || content.is_empty() {
            if let Some(last) = statements.last_mut() {
                last.lines.push(line);
            }
            continue;
        }
        if let Some(rest) = content.strip_prefix('+')
            && let Some(last) = statements.last_mut().filter(|s| !s.encrypted)
        {
            last.text.push(' ');
            last.text.push_str(rest);
            last.lines.push(line);
            continue;
        }
        statements.push(Statement {
            text: content.to_string(),
            lines: vec![line],
            line: index + 1,
            encrypted: false,
        });
    }
    statements
}

fn is_comment_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('*') || strip_comment(trimmed).trim().is_empty()
}

/// Removes an inline comment: `;`, `//`, or a `$` after whitespace and followed by whitespace
/// (ngspice's `$` comment; `$G_DPWR` is a node name in PSpice and stays).
fn strip_comment(line: &str) -> &str {
    let mut end = line.len();
    if let Some(i) = line.find(';') {
        end = end.min(i);
    }
    if let Some(i) = line.find("//") {
        end = end.min(i);
    }
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'$'
            && (i == 0 || bytes[i - 1].is_ascii_whitespace())
            && bytes
                .get(i + 1)
                .is_none_or(|next| next.is_ascii_whitespace())
        {
            end = end.min(i);
            break;
        }
    }
    &line[..end]
}

fn keyword(text: &str) -> &str {
    text.split_whitespace().next().unwrap_or_default()
}

/// Splits a statement into tokens: whitespace separates them, except inside `{...}`, `'...'`
/// and `"..."`; `name = value` is joined into `name=value`.
fn tokenize(text: &str) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                current.push(c);
            }
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                current.push(c);
            }
            (None, '{') => {
                depth += 1;
                current.push(c);
            }
            (None, '}') => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            (None, c) if c.is_whitespace() && depth == 0 => {
                if !current.is_empty() {
                    raw.push(std::mem::take(&mut current));
                }
            }
            (None, c) => current.push(c),
        }
    }
    if !current.is_empty() {
        raw.push(current);
    }
    let mut tokens: Vec<String> = Vec::new();
    let mut join_next = false;
    for token in raw {
        if (join_next || token.starts_with('='))
            && let Some(last) = tokens.last_mut()
        {
            last.push_str(&token);
            join_next = token == "=" || (token.ends_with('=') && !token.contains("=="));
            continue;
        }
        join_next = token.ends_with('=') && !token.ends_with("==");
        tokens.push(token);
    }
    tokens
}

/// Parses the tokens after `.subckt NAME`.
fn subckt_header(tokens: &[String]) -> DefinitionKind {
    #[derive(PartialEq)]
    enum Part {
        Ports,
        Optional,
        Params,
    }
    let mut part = Part::Ports;
    let mut ports = Vec::new();
    let mut optional_ports = Vec::new();
    let mut params = Vec::new();
    for token in tokens {
        let lower = token.to_ascii_lowercase();
        let mut token = token.as_str();
        for (keyword, next) in [
            ("optional:", Part::Optional),
            ("params:", Part::Params),
            ("text:", Part::Params),
        ] {
            if lower.starts_with(keyword) {
                part = next;
                token = &token[keyword.len()..];
                break;
            }
        }
        if token.is_empty() {
            continue;
        }
        if part == Part::Ports && token.contains('=') {
            part = Part::Params;
        }
        let pair = || {
            let (name, value) = token.split_once('=').unwrap_or((token, ""));
            (name.to_string(), value.to_string())
        };
        match part {
            Part::Ports => ports.push(token.to_string()),
            Part::Optional => optional_ports.push(pair()),
            Part::Params => params.push(pair()),
        }
    }
    DefinitionKind::Subckt {
        ports,
        optional_ports,
        params,
    }
}

fn unquote(text: &str) -> &str {
    text.trim_matches(['"', '\''])
}

/// Whether lowercase `text` calls `function` (`if(`, `if (`), not as part of a longer name.
fn calls(text: &str, function: &str) -> bool {
    let is_name = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(function).any(|(i, _)| {
        !text[..i].ends_with(is_name) && text[i + function.len()..].trim_start().starts_with('(')
    })
}

/// Whether `text` has `op` not doubled (`&` but not `&&`).
fn single(text: &str, op: char) -> bool {
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len()).any(|i| {
        chars[i] == op
            && (i == 0 || chars[i - 1] != op)
            && chars.get(i + 1).is_none_or(|&c| c != op)
    })
}

fn join(lines: &[&str]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Decodes a text file: UTF-8, or Windows-1252 if it is not valid UTF-8 (vendor models often
/// have `µ`, `°` or `Ω` in comments in that encoding).
pub fn decode(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => e.into_bytes().iter().map(|&b| windows_1252(b)).collect(),
    }
}

fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9f => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIB: &str = "\
* A test library
.subckt amp in+ in- out params: GAIN=10
+ ROUT=1
* the gain stage
E1 out 0 in+ in- {GAIN}
.subckt inner a b
R1 a b 1k
.ends inner
.model Dlimit D N=0.01
.ends amp

.MODEL D1N4148 D(IS=2.52n RS=0.568
+ N=1.752) ; fitted
.subckt varistor A B threshold=1k
.ends
.include \"other.lib\"
";

    type Header<'a> = (&'a [String], &'a [(String, String)], &'a [(String, String)]);

    fn subckt(def: &Definition) -> Header<'_> {
        match &def.kind {
            DefinitionKind::Subckt {
                ports,
                optional_ports,
                params,
            } => (ports, optional_ports, params),
            other => panic!("{other:?}"),
        }
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn test_definitions() {
        let lib = SpiceLib::parse(LIB).unwrap();
        let names: Vec<_> = lib.definitions.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["amp", "D1N4148", "varistor"]);
        let amp = lib.find("AMP").unwrap();
        let (ports, _, params) = subckt(amp);
        assert_eq!(ports, ["in+", "in-", "out"]);
        assert_eq!(params, pairs(&[("GAIN", "10"), ("ROUT", "1")]));
        assert_eq!(amp.line, 2);
        assert!(amp.text.starts_with(".subckt amp"));
        assert!(amp.text.ends_with(".ends amp\n"));
        assert!(amp.text.contains(".ends inner"));
        let diode = lib.find("d1n4148").unwrap();
        assert_eq!(
            diode.kind,
            DefinitionKind::Model {
                model_type: "d".into()
            }
        );
        assert_eq!(
            diode.text,
            ".MODEL D1N4148 D(IS=2.52n RS=0.568\n+ N=1.752) ; fitted\n"
        );
        let (ports, _, params) = subckt(lib.find("varistor").unwrap());
        assert_eq!(ports, ["A", "B"]);
        assert_eq!(params, pairs(&[("threshold", "1k")]));
        assert!(
            lib.find("Dlimit").is_none(),
            "models inside a subcircuit are local"
        );
        assert_eq!(
            lib.includes
                .iter()
                .map(|i| i.path.as_str())
                .collect::<Vec<_>>(),
            ["other.lib"]
        );
        assert!(lib.hints.is_empty());
    }

    #[test]
    fn test_pspice_header() {
        let lib = SpiceLib::parse(
            "*$\n.SUBCKT SUP CTR CTS GND RESET_B SENSE VDD\n+ OPTIONAL: DPWR=$G_DPWR DGND = $G_DGND\n\
             + PARAMS: VTH = {0.5 * 2} TD=1m\n+ TEXT: NAME=\"x y\"\n.ENDS SUP\n",
        )
        .unwrap();
        let (ports, optional, params) = subckt(lib.find("sup").unwrap());
        assert_eq!(ports, ["CTR", "CTS", "GND", "RESET_B", "SENSE", "VDD"]);
        assert_eq!(optional, pairs(&[("DPWR", "$G_DPWR"), ("DGND", "$G_DGND")]));
        assert_eq!(
            params,
            pairs(&[("VTH", "{0.5 * 2}"), ("TD", "1m"), ("NAME", "\"x y\"")])
        );
    }

    #[test]
    fn test_params_funcs_and_comments() {
        let lib = SpiceLib::parse(
            ".PARAM VREF=1.2 VHYS = {VREF/100}\n.FUNC clamp(x) {min(max(x, 0), 1)}\n\
             .subckt s a b $ ngspice comment\n.ends\n",
        )
        .unwrap();
        let kinds: Vec<_> = lib
            .definitions
            .iter()
            .map(|d| (d.name.as_str(), d.text.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("VREF", ".param VREF=1.2\n"),
                ("VHYS", ".param VHYS={VREF/100}\n"),
                ("clamp", ".FUNC clamp(x) {min(max(x, 0), 1)}\n"),
                ("s", ".subckt s a b $ ngspice comment\n.ends\n"),
            ]
        );
        assert_eq!(subckt(lib.find("s").unwrap()).0, ["a", "b"]);
        assert!(
            lib.find("VREF").is_none(),
            "find looks for models and subcircuits only"
        );
    }

    #[test]
    fn test_includes_and_sections() {
        let lib = SpiceLib::parse(".LIB \"vendor.lib\"\n.lib 'corner.lib' TT\n").unwrap();
        assert_eq!(
            lib.includes,
            vec![
                Include {
                    path: "vendor.lib".into(),
                    section: None,
                    within: None,
                    pspice: true
                },
                Include {
                    path: "corner.lib".into(),
                    section: Some("TT".into()),
                    within: None,
                    pspice: false
                },
            ]
        );
        assert_eq!(lib.hints[0].what, ".LIB file");

        let lib = SpiceLib::parse(
            ".lib TT\n.param corner=0\n.model n NMOS\n.include tt.lib\n.endl TT\n.lib FF\n.model n NMOS\n.endl\n",
        )
        .unwrap();
        assert_eq!(lib.sections, ["TT", "FF"]);
        assert!(lib.find("n").is_none());
        assert_eq!(lib.find_in("N", Some("ff")).unwrap().line, 7);
        assert_eq!(lib.includes(Some("tt")).count(), 1);
        assert_eq!(lib.includes(None).count(), 0);
        assert!(lib.hints.is_empty());
    }

    #[test]
    fn test_hints() {
        let lib = SpiceLib::parse(
            ".subckt s a b\nE1 b 0 VALUE = { IF(V(a) > 1 & V(a) < 2, 1, 0) }\n\
             E2 c 0 VALUE = { LIMIT(V(a), 0, 1) && 1 }\nB1 d 0 V=uplim(V(a), 1, 0.1)\n* IF(x) | y\n.ends\n",
        )
        .unwrap();
        let hints: Vec<_> = lib
            .hints
            .iter()
            .map(|h| (h.dialect, h.what, h.line))
            .collect();
        assert_eq!(
            hints,
            [
                (Dialect::Pspice, "IF(", 2),
                (Dialect::Pspice, "&", 2),
                (Dialect::Pspice, "LIMIT(", 3),
                (Dialect::Ltspice, "uplim(", 4),
            ]
        );
        assert!(!calls("ifx(1)", "if") && !calls("dif(1)", "if") && calls("a+if (1)", "if"));
    }

    #[test]
    fn test_encrypted() {
        let lib = SpiceLib::parse(
            ".subckt s a b\n$CDNENCSTART\n.subckt fake\n0123abcd\n$CDNENCFINISH\n.ends s\n\
             $CDNENCSTART\nffee\n$CDNENCFINISH\n.model d D\n",
        )
        .unwrap();
        let s = lib.find("s").unwrap();
        assert!(s.encrypted);
        assert!(s.text.contains("0123abcd"));
        assert!(!lib.find("d").unwrap().encrypted);
        assert_eq!(lib.encrypted_blocks, 1);
        assert_eq!(lib.definitions.len(), 2);
    }

    #[test]
    fn test_errors() {
        let error = |text: &str| SpiceLib::parse(text).unwrap_err().to_string();
        assert_eq!(
            error(".subckt a x\n.ends b\n"),
            "line 2: `.ends b` closes `.subckt a`"
        );
        assert_eq!(
            error(".subckt a x\n"),
            "`.subckt a` is not closed with `.ends`"
        );
        assert_eq!(error(".ends\n"), "line 1: `.ends` without `.subckt`");
        assert_eq!(error(".endl\n"), "line 1: `.endl` outside a section");
    }

    #[test]
    fn test_vendor_style_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/models/vsup01.lib"
        );
        let text = decode(std::fs::read(path).unwrap());
        assert!(text.contains("0 \u{b5}s"), "Windows-1252 is decoded");
        let lib = SpiceLib::parse(&text).unwrap();
        let sup = lib.find("vsup01").unwrap();
        let (ports, optional, params) = subckt(sup);
        assert_eq!(ports, ["CTR", "CTS", "GND", "RESET_B", "SENSE", "VDD"]);
        assert!(optional.is_empty());
        assert_eq!(params, pairs(&[("VTH", "0.5"), ("VDDMIN", "0.8")]));
        assert!(sup.text.ends_with(".ENDS VSUP01\n") && !sup.text.contains('\r'));
        let names: Vec<_> = lib.definitions.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["VSUP_RON", "VSUP_DRIVE", "VSUP01", "D_VSUP_ESD"]);
        assert_eq!(lib.includes[0].path, "vsup01_aux.lib");
        assert!(lib.includes[0].pspice);
        let hints: Vec<_> = lib.hints.iter().map(|h| (h.dialect, h.what)).collect();
        assert_eq!(
            hints,
            [
                (Dialect::Pspice, ".LIB file"),
                (Dialect::Pspice, "IF("),
                (Dialect::Pspice, "&")
            ]
        );

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/models/vsup_unsupported.lib"
        );
        let lib = SpiceLib::parse(&decode(std::fs::read(path).unwrap())).unwrap();
        assert_eq!(subckt(lib.find("VSUP_DIGITAL").unwrap()).1.len(), 2);
        assert!(lib.find("VSUP_LOCKED").unwrap().encrypted);
    }

    #[test]
    fn test_decode() {
        assert_eq!(decode(b"1\xb5F \x80\r\n".to_vec()), "1µF €\r\n");
        assert_eq!(decode("µ".as_bytes().to_vec()), "µ");
        // CRLF line ends are handled by `str::lines`.
        let lib = SpiceLib::parse(".subckt a x y\r\n.ends a\r\n").unwrap();
        assert_eq!(subckt(lib.find("a").unwrap()).0, ["x", "y"]);
        assert_eq!(lib.find("a").unwrap().text, ".subckt a x y\n.ends a\n");
    }
}
