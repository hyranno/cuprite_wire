//! Definitions in SPICE model libraries (`.lib`, `.sp`, `.sub`, ...).
//!
//! Only what is needed to use a model from a part is read: the top-level `.subckt`
//! definitions with their ports and the top-level `.model` statements with their types,
//! together with their text so they can be copied into a deck.

/// A top-level definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub name: String,
    pub kind: DefinitionKind,
    /// The definition as written in the library, from the `.subckt` / `.model` line to
    /// `.ends` / the last continuation line, ending with a newline.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionKind {
    /// `.subckt NAME port... [params:] [name=value...]`
    Subckt { ports: Vec<String> },
    /// `.model NAME TYPE ...`; the type is lowercased (`d`, `npn`, `vdmos`, ...).
    Model { model_type: String },
}

/// The definitions of a library file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpiceLib {
    pub definitions: Vec<Definition>,
    /// Files named by top-level `.include` / `.inc` lines, as written.
    pub includes: Vec<String>,
}

impl SpiceLib {
    pub fn parse(text: &str) -> Self {
        let mut lib = SpiceLib::default();
        // Physical lines grouped into logical lines (continuations start with '+').
        let mut logical: Vec<(String, Vec<&str>)> = Vec::new();
        for line in text.lines() {
            let content = strip_comment(line);
            let trimmed = content.trim();
            if line.trim_start().starts_with('*') || trimmed.is_empty() {
                if let Some(last) = logical.last_mut() {
                    last.1.push(line);
                }
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix('+')
                && let Some(last) = logical.last_mut()
            {
                last.0.push(' ');
                last.0.push_str(rest);
                last.1.push(line);
                continue;
            }
            logical.push((trimmed.to_string(), vec![line]));
        }

        let mut depth = 0usize;
        let mut current: Option<(String, Vec<String>, Vec<&str>)> = None;
        for (line, physical) in &logical {
            let mut tokens = line.split_whitespace();
            let keyword = tokens.next().unwrap_or_default().to_ascii_lowercase();
            // Trailing comment-only lines belong to the next statement, not to this one.
            let own_lines = || {
                let end = physical
                    .iter()
                    .rposition(|l| !l.trim_start().starts_with('*') && !l.trim().is_empty())
                    .map_or(1, |i| i + 1);
                &physical[..end]
            };
            if let Some((_, _, lines)) = &mut current {
                lines.extend(own_lines());
            }
            match keyword.as_str() {
                ".subckt" => {
                    if depth == 0 {
                        let name = tokens.next().unwrap_or_default().to_string();
                        let ports = tokens
                            .take_while(|t| !t.contains('=') && !t.eq_ignore_ascii_case("params:"))
                            .map(str::to_string)
                            .collect();
                        current = Some((name, ports, own_lines().to_vec()));
                    }
                    depth += 1;
                }
                ".ends" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0
                        && let Some((name, ports, lines)) = current.take()
                    {
                        lib.definitions.push(Definition {
                            name,
                            kind: DefinitionKind::Subckt { ports },
                            text: join(&lines),
                        });
                    }
                }
                ".model" if depth == 0 => {
                    let name = tokens.next().unwrap_or_default().to_string();
                    let model_type = tokens
                        .next()
                        .unwrap_or_default()
                        .split('(')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    lib.definitions.push(Definition {
                        name,
                        kind: DefinitionKind::Model { model_type },
                        text: join(own_lines()),
                    });
                }
                ".include" | ".inc" if depth == 0 => {
                    let rest = line[keyword.len()..].trim();
                    lib.includes.push(rest.trim_matches(['"', '\'']).to_string());
                }
                _ => {}
            }
        }
        lib
    }

    /// The definition named `name` (case-insensitively, like SPICE).
    pub fn find(&self, name: &str) -> Option<&Definition> {
        self.definitions
            .iter()
            .find(|definition| definition.name.eq_ignore_ascii_case(name))
    }
}

/// Removes an inline comment (`;`, `$` after whitespace, `//`).
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
        if *b == b'$' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            end = end.min(i);
            break;
        }
    }
    &line[..end]
}

fn join(lines: &[&str]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
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

    #[test]
    fn test_definitions() {
        let lib = SpiceLib::parse(LIB);
        let names: Vec<_> = lib.definitions.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["amp", "D1N4148", "varistor"]);
        let amp = lib.find("AMP").unwrap();
        assert_eq!(
            amp.kind,
            DefinitionKind::Subckt {
                ports: vec!["in+".into(), "in-".into(), "out".into()]
            }
        );
        assert!(amp.text.starts_with(".subckt amp"));
        assert!(amp.text.ends_with(".ends amp\n"));
        assert!(amp.text.contains(".ends inner"));
        let diode = lib.find("d1n4148").unwrap();
        assert_eq!(diode.kind, DefinitionKind::Model { model_type: "d".into() });
        assert_eq!(diode.text, ".MODEL D1N4148 D(IS=2.52n RS=0.568\n+ N=1.752) ; fitted\n");
        assert_eq!(
            lib.find("varistor").unwrap().kind,
            DefinitionKind::Subckt {
                ports: vec!["A".into(), "B".into()]
            }
        );
        assert!(lib.find("Dlimit").is_none(), "models inside a subcircuit are local");
        assert_eq!(lib.includes, ["other.lib"]);
    }
}
