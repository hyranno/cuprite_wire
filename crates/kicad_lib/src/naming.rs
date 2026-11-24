//! Names and identifiers shared with `cuprite_wire`: library identifiers, natural ordering,
//! symbol unit letters and SPICE element suffixes.

use std::{cmp::Ordering, fmt, str::FromStr};

/// A KiCad library identifier, `Library:Name` (e.g. `Resistor_SMD:R_0603_1608Metric`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LibId {
    library: String,
    name: String,
}

/// An invalid library identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibIdError {
    /// The identifier as given, `Library:Name`.
    pub lib_id: String,
    /// Why it is invalid.
    pub reason: &'static str,
}

impl fmt::Display for LibIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid library identifier {:?}: {}",
            self.lib_id, self.reason
        )
    }
}

impl std::error::Error for LibIdError {}

impl From<LibIdError> for crate::Error {
    fn from(error: LibIdError) -> Self {
        crate::Error::new(error.to_string())
    }
}

impl LibId {
    /// Makes an identifier from the library and item names.
    ///
    /// # Errors
    /// If a name is empty or contains `:` or a control character.
    pub fn new(library: impl Into<String>, name: impl Into<String>) -> Result<Self, LibIdError> {
        let library = library.into();
        let name = name.into();
        let error = |reason| {
            Err(LibIdError {
                lib_id: format!("{library}:{name}"),
                reason,
            })
        };
        if library.is_empty() {
            return error("the library name is empty");
        }
        if name.is_empty() {
            return error("the item name is empty");
        }
        if library.contains(':') || name.contains(':') {
            return error("names must not contain ':'");
        }
        if library.chars().chain(name.chars()).any(char::is_control) {
            return error("names must not contain control characters");
        }
        Ok(LibId { library, name })
    }

    /// The library name, the part before `:`.
    pub fn library(&self) -> &str {
        &self.library
    }

    /// The item name, the part after `:`.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl FromStr for LibId {
    type Err = LibIdError;

    /// Parses `Library:Name`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once(':') {
            Some((library, name)) => LibId::new(library, name),
            None => Err(LibIdError {
                lib_id: s.to_string(),
                reason: "expected the form 'Library:Name'",
            }),
        }
    }
}

impl fmt::Display for LibId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.library, self.name)
    }
}

/// Compares strings in natural order: digit runs are compared as numbers (`R2` < `R10`).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    fn chunks(s: &str) -> impl Iterator<Item = (bool, &str)> {
        let mut rest = s;
        std::iter::from_fn(move || {
            let first = rest.chars().next()?;
            let is_digit = first.is_ascii_digit();
            let end = rest
                .find(|c: char| c.is_ascii_digit() != is_digit)
                .unwrap_or(rest.len());
            let (chunk, tail) = rest.split_at(end);
            rest = tail;
            Some((is_digit, chunk))
        })
    }
    let mut a_chunks = chunks(a);
    let mut b_chunks = chunks(b);
    loop {
        let ordering = match (a_chunks.next(), b_chunks.next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some((true, x)), Some((true, y))) => {
                let x = x.trim_start_matches('0');
                let y = y.trim_start_matches('0');
                x.len().cmp(&y.len()).then_with(|| x.cmp(y))
            }
            (Some((_, x)), Some((_, y))) => x.cmp(y),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
}

/// The letters KiCad appends to the reference of unit `unit` (from 1) of a multi-unit symbol:
/// `A` to `Z`, then `AA`, `AB`, ... (`U1A`, `U1AA`).
///
/// # Panics
/// If `unit` is 0.
pub fn unit_letters(unit: u32) -> String {
    assert!(unit > 0, "units are numbered from 1");
    let mut letters = Vec::new();
    let mut rest = unit;
    while rest > 0 {
        let digit = (rest - 1) % 26;
        letters.push(b'A' + digit as u8);
        rest = (rest - 1 - digit) / 26;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap()
}

/// Checks the suffix appended to a part's name to name one of its SPICE elements: ASCII letters,
/// digits and `_` (it may be empty).
pub fn check_element_suffix(suffix: &str) -> Result<(), String> {
    if suffix
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        Ok(())
    } else {
        Err(format!(
            "suffix {suffix:?} may only contain ASCII letters, digits and '_'"
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_unit_letters() {
        let letters: Vec<_> = [1, 2, 26, 27, 28, 52, 53, 702, 703]
            .into_iter()
            .map(super::unit_letters)
            .collect();
        assert_eq!(
            letters,
            ["A", "B", "Z", "AA", "AB", "AZ", "BA", "ZZ", "AAA"]
        );
    }

    use super::*;

    #[test]
    fn test_parse_lib_id() {
        let lib_id: LibId = "Resistor_SMD:R_0603_1608Metric".parse().unwrap();
        assert_eq!(lib_id.library(), "Resistor_SMD");
        assert_eq!(lib_id.name(), "R_0603_1608Metric");
        assert_eq!(lib_id.to_string(), "Resistor_SMD:R_0603_1608Metric");
    }

    #[test]
    fn test_invalid_lib_ids() {
        for invalid in ["R_0603", ":R_0603", "Resistor_SMD:", "A:B:C", "A:B\n"] {
            assert!(
                invalid.parse::<LibId>().is_err(),
                "{invalid:?} should be rejected"
            );
        }
    }

    #[test]
    fn test_natural_cmp() {
        let mut names = vec![
            "R10",
            "R2",
            "R1",
            "C1",
            "R02",
            "/a/OUT",
            "Net-(R1-Pad2)",
            "GND",
            "R",
            "10",
            "2",
            "1",
            "",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            vec![
                "",
                "/a/OUT",
                "1",
                "2",
                "10",
                "C1",
                "GND",
                "Net-(R1-Pad2)",
                "R",
                "R1",
                "R02",
                "R2",
                "R10"
            ]
        );
    }

    #[test]
    fn test_check_element_suffix() {
        for valid in ["", "A", "q_1"] {
            assert_eq!(check_element_suffix(valid), Ok(()));
        }
        for invalid in ["a-b", "x y", "é"] {
            assert!(check_element_suffix(invalid).is_err(), "{invalid:?}");
        }
    }
}
