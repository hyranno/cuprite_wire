//! A minimal S-expression writer for KiCad-style files.
//!
//! Only generation is supported. Strings are always quoted, and lists are
//! pretty-printed with two-space indentation so that the output is easy to diff.

use std::fmt::{self, Display, Write};

use cuprite_wire_kicad_lib::sexpr::ESCAPES;

/// An S-expression node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SExpr {
    /// `( ... )`
    List(Vec<SExpr>),
    /// A bare token such as a keyword (`comp`, `ref`, ...).
    Symbol(&'static str),
    /// A string, always written in double quotes.
    Str(String),
}

impl SExpr {
    /// `(name child...)`
    pub fn node(name: &'static str, children: impl IntoIterator<Item = SExpr>) -> SExpr {
        let mut items = vec![SExpr::Symbol(name)];
        items.extend(children);
        SExpr::List(items)
    }

    /// `(name "value")`
    pub fn kv(name: &'static str, value: impl Into<String>) -> SExpr {
        SExpr::node(name, [SExpr::string(value)])
    }

    /// A quoted string atom.
    pub fn string(value: impl Into<String>) -> SExpr {
        SExpr::Str(value.into())
    }

    fn is_atom(&self) -> bool {
        !matches!(self, SExpr::List(_))
    }

    /// Nesting depth: atoms are 0, a list of atoms is 1, and so on.
    fn depth(&self) -> usize {
        match self {
            SExpr::List(items) => 1 + items.iter().map(SExpr::depth).max().unwrap_or(0),
            _ => 0,
        }
    }

    fn write_inline(&self, out: &mut impl Write) -> fmt::Result {
        match self {
            SExpr::Symbol(symbol) => out.write_str(symbol),
            SExpr::Str(value) => write_quoted(out, value),
            SExpr::List(items) => {
                out.write_char('(')?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.write_char(' ')?;
                    }
                    item.write_inline(out)?;
                }
                out.write_char(')')
            }
        }
    }

    /// Lists nested at most two levels deep are written on one line.
    /// Deeper lists write their leading atoms, followed by the next child if
    /// it is a list of atoms, on the first line and each remaining child on
    /// its own indented line, as KiCad does (e.g. `(export (version "E")`).
    /// pcbnew relies on this: it detects a netlist by a line starting with
    /// `(export ` and rejects the file otherwise.
    fn write_pretty(&self, out: &mut impl Write, indent: usize) -> fmt::Result {
        let SExpr::List(items) = self else {
            return self.write_inline(out);
        };
        if self.depth() <= 2 {
            return self.write_inline(out);
        }
        out.write_char('(')?;
        let mut head_len = items.iter().take_while(|item| item.is_atom()).count();
        if head_len > 0 && items.get(head_len).is_some_and(|item| item.depth() == 1) {
            head_len += 1;
        }
        for (i, item) in items[..head_len].iter().enumerate() {
            if i > 0 {
                out.write_char(' ')?;
            }
            item.write_inline(out)?;
        }
        for item in &items[head_len..] {
            out.write_char('\n')?;
            for _ in 0..indent + 1 {
                out.write_str("  ")?;
            }
            item.write_pretty(out, indent + 1)?;
        }
        out.write_char(')')
    }
}

impl Display for SExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_pretty(f, 0)
    }
}

fn write_quoted(out: &mut impl Write, value: &str) -> fmt::Result {
    out.write_char('"')?;
    for c in value.chars() {
        match ESCAPES.iter().find(|(_, unescaped)| *unescaped == c) {
            Some(&(escaped, _)) => {
                out.write_char('\\')?;
                out.write_char(escaped)?;
            }
            None => out.write_char(c)?,
        }
    }
    out.write_char('"')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strings_are_quoted_and_escaped() {
        let expr = SExpr::node(
            "value",
            [
                SExpr::string(r#"c="1" \ x"#),
                SExpr::string("10"),
                SExpr::string(""),
            ],
        );
        assert_eq!(expr.to_string(), r#"(value "c=\"1\" \\ x" "10" "")"#);
    }

    #[test]
    fn test_control_characters_are_escaped() {
        assert_eq!(SExpr::string("a\nb\tc").to_string(), r#""a\nb\tc""#);
    }

    #[test]
    fn test_shallow_lists_are_inline() {
        let expr = SExpr::node(
            "node",
            [
                SExpr::kv("ref", "R1"),
                SExpr::kv("pin", "2"),
                SExpr::kv("pintype", "passive"),
            ],
        );
        assert_eq!(
            expr.to_string(),
            r#"(node (ref "R1") (pin "2") (pintype "passive"))"#
        );
    }

    #[test]
    fn test_deep_lists_are_indented() {
        let expr = SExpr::node(
            "export",
            [
                SExpr::kv("version", "E"),
                SExpr::node(
                    "components",
                    [SExpr::node(
                        "comp",
                        [
                            SExpr::kv("ref", "R1"),
                            SExpr::node(
                                "fields",
                                [SExpr::node(
                                    "field",
                                    [SExpr::kv("name", "A"), SExpr::string("x")],
                                )],
                            ),
                        ],
                    )],
                ),
                SExpr::node("nets", []),
            ],
        );
        let expected = concat!(
            "(export (version \"E\")\n",
            "  (components\n",
            "    (comp (ref \"R1\")\n",
            "      (fields\n",
            "        (field (name \"A\") \"x\"))))\n",
            "  (nets))",
        );
        assert_eq!(expr.to_string(), expected);
    }

    #[test]
    fn test_leading_atoms_stay_on_first_line() {
        let expr = SExpr::List(vec![
            SExpr::Symbol("a"),
            SExpr::string("b"),
            SExpr::node("c", [SExpr::node("d", [SExpr::kv("e", "f")])]),
        ]);
        assert_eq!(expr.to_string(), "(a \"b\"\n  (c\n    (d (e \"f\"))))");
    }

    #[test]
    fn test_kicad_lib_reads_written_strings() {
        let value = "a \"b\" \\ c\nd\re\tf (g)";
        let text = SExpr::string(value).to_string();
        let read = cuprite_wire_kicad_lib::sexpr::parse(&text).unwrap();
        assert_eq!(read.text(), Some(value));
    }
}
