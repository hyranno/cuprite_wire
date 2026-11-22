//! Splits an IBIS file into keywords with the lines that follow them, without comments.

use crate::Error;

/// The characters `[Comment Char]` may select.
const COMMENT_CHARS: &str = "!\"#$%&'()*,:;<>?@\\^`{|}~";

/// A keyword (`[Model] Output`) and the lines up to the next keyword.
#[derive(Debug)]
pub(crate) struct Block {
    /// The keyword as written, with the brackets (`[GND_clamp]`).
    pub keyword: String,
    /// The keyword in lowercase, with underscores and runs of spaces as one space
    /// (`gnd clamp`): keywords are case-insensitive and treat `_` and ` ` alike.
    pub name: String,
    /// The text after the keyword on its line, trimmed.
    pub args: String,
    /// The line number (1-based) of the keyword.
    pub line: usize,
    /// The following non-empty lines, trimmed.
    pub body: Vec<Line>,
}

/// A line without its comment, trimmed.
#[derive(Debug)]
pub(crate) struct Line {
    pub line: usize,
    pub text: String,
}

impl Line {
    pub fn tokens(&self) -> impl Iterator<Item = &str> {
        self.text.split_whitespace()
    }
}

/// The lowercase name of a keyword, with `_` and runs of spaces as one space.
pub(crate) fn normalize(keyword: &str) -> String {
    keyword
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Splits the text into keywords, up to `[END]` (excluded). Returns the blocks and the comment
/// character in effect at the end.
pub(crate) fn lex(text: &str) -> Result<(Vec<Block>, char), Error> {
    let mut comment = '|';
    let mut blocks: Vec<Block> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim_start();
        if let Some(inner) = trimmed.strip_prefix('[') {
            let close = inner
                .find(']')
                .ok_or_else(|| Error::new(line, "keyword without a closing `]`"))?;
            let keyword = &inner[..close];
            let name = normalize(keyword);
            let rest = &inner[close + 1..];
            let args = if name == "comment char" {
                // The argument may be the old comment character itself (`|_char`).
                comment = comment_char(rest, line)?;
                comment.to_string()
            } else {
                strip_comment(rest, comment).trim().to_string()
            };
            if name == "end" {
                break;
            }
            blocks.push(Block {
                keyword: format!("[{}]", keyword.trim()),
                name,
                args,
                line,
                body: Vec::new(),
            });
        } else {
            let text = strip_comment(raw, comment).trim();
            if text.is_empty() {
                continue;
            }
            let block = blocks
                .last_mut()
                .ok_or_else(|| Error::new(line, "text before the first keyword"))?;
            block.body.push(Line {
                line,
                text: text.to_string(),
            });
        }
    }
    Ok((blocks, comment))
}

fn strip_comment(text: &str, comment: char) -> &str {
    text.split(comment).next().unwrap_or_default()
}

/// The character of `[Comment Char] <c>_char`.
fn comment_char(args: &str, line: usize) -> Result<char, Error> {
    let arg = args.split_whitespace().next().unwrap_or_default();
    let mut chars = arg.chars();
    match (chars.next(), chars.as_str()) {
        (Some(c), suffix) if suffix.eq_ignore_ascii_case("_char") && COMMENT_CHARS.contains(c) => {
            Ok(c)
        }
        _ => Err(Error::new(
            line,
            format!(
                "invalid [Comment Char] `{arg}`; expected `<c>_char` with one of {COMMENT_CHARS}"
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lex() {
        let text = "\
| leading comment
[IBIS Ver] 2.1 | comment
[Comment Char] #_char
[File_Name]   x.ibs # comment | not a comment
[GND  clamp]
#  V  I
  -5.0V  -50m   # row comment

  0  0
[END]
[Model] after the end
";
        let (blocks, comment) = lex(text).unwrap();
        assert_eq!(comment, '#');
        let summary: Vec<_> = blocks
            .iter()
            .map(|b| (b.keyword.as_str(), b.name.as_str(), b.args.as_str(), b.line))
            .collect();
        assert_eq!(
            summary,
            [
                ("[IBIS Ver]", "ibis ver", "2.1", 2),
                ("[Comment Char]", "comment char", "#", 3),
                ("[File_Name]", "file name", "x.ibs", 4),
                ("[GND  clamp]", "gnd clamp", "", 5),
            ]
        );
        let body: Vec<_> = blocks[3]
            .body
            .iter()
            .map(|l| (l.line, l.text.as_str()))
            .collect();
        assert_eq!(body, [(7, "-5.0V  -50m"), (9, "0  0")]);
    }

    #[test]
    fn test_lex_errors() {
        let err = lex("[IBIS Ver 2.1\n").unwrap_err();
        assert_eq!(err.line, 1);
        let err = lex("text\n[IBIS Ver] 2.1\n").unwrap_err();
        assert_eq!(
            (err.line, err.message.as_str()),
            (1, "text before the first keyword")
        );
        let err = lex("[IBIS Ver] 2.1\n[Comment Char] a_char\n").unwrap_err();
        assert_eq!(err.line, 2);
        let err = lex("[IBIS Ver] 2.1\n[Comment Char] #\n").unwrap_err();
        assert_eq!(err.line, 2);
    }
}
