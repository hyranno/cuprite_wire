//! Numbers in KiCad's notation, converted to SPICE's.
//!
//! KiCad's simulator reads values with SI prefixes, so `M` is mega (SPICE reads it as milli), and
//! the `Value` field may use the RKM code (`4k7`, `2R2`, `1n5`), which SPICE reads only up to the
//! letter (`4k7` is 4k). KiCad converts both when it writes a SPICE netlist; so does this module.

/// SI prefixes: the letter, the power of ten, and how SPICE writes it (`None`: as an exponent).
const PREFIXES: &[(char, i32, Option<&str>)] = &[
    ('a', -18, None),
    ('f', -15, Some("f")),
    ('p', -12, Some("p")),
    ('n', -9, Some("n")),
    ('u', -6, Some("u")),
    ('µ', -6, Some("u")),
    ('μ', -6, Some("u")),
    ('m', -3, Some("m")),
    ('k', 3, Some("k")),
    ('K', 3, Some("k")),
    ('M', 6, Some("Meg")),
    ('G', 9, Some("G")),
    ('T', 12, Some("T")),
    ('P', 15, None),
    ('E', 18, None),
];

/// Letters that may stand for the decimal point (`4k7`); `R` is the point without a prefix.
/// `a`, `P` and `E` are left out: `1E5` is an exponent.
const RKM_LETTERS: &str = "fpnuµμmkKMGTR";

/// The units that may follow a value of a device.
fn units(device: char) -> &'static [&'static str] {
    match device {
        'R' => &["Ω", "Ω", "ohm", "R"],
        'C' => &["F"],
        'L' => &["H"],
        'V' => &["V"],
        'I' => &["A"],
        _ => &[],
    }
}

/// [`to_spice`], with an error if the text is not a number.
pub fn try_to_spice(text: &str, device: char) -> Result<String, crate::Error> {
    to_spice(text, device).ok_or_else(|| {
        crate::Error::new(format!(
            "the value {text:?} is not a number (like 10k, 4k7 or 1M for mega)"
        ))
    })
}

/// Converts a number in KiCad's notation (`10k`, `1M`, `4k7`, `2R2`, `1.5e-9`, `100nF`) to SPICE
/// (`10k`, `1Meg`, `4.7k`, `2.2`, `1.5e-9`, `100n`). `None` if the text is not such a number.
///
/// `device` is the SPICE device letter of the element, which selects the unit that may follow
/// (`Ω` / `ohm` / `R` for `R`, `F` for `C`, `H` for `L`, `V` for `V`, `A` for `I`).
pub fn to_spice(text: &str, device: char) -> Option<String> {
    let mut rest = text.trim();
    let mut out = String::new();
    if let Some(r) = rest.strip_prefix('-') {
        out.push('-');
        rest = r;
    } else if let Some(r) = rest.strip_prefix('+') {
        rest = r;
    }
    let int = take_digits(&mut rest);
    let mut frac = "";
    let mut prefix = None;
    let mut rkm = false;
    if let Some(r) = rest.strip_prefix('.') {
        rest = r;
        frac = take_digits(&mut rest);
    } else if let Some(letter) = rest.chars().next()
        && !int.is_empty()
        && RKM_LETTERS.contains(letter)
        && rest[letter.len_utf8()..].starts_with(|c: char| c.is_ascii_digit())
    {
        // RKM: the letter is the decimal point and the prefix.
        rest = &rest[letter.len_utf8()..];
        frac = take_digits(&mut rest);
        prefix = Some(letter).filter(|&l| l != 'R');
        rkm = true;
    }
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    let mut exponent = None;
    if !rkm && let Some(r) = rest.strip_prefix(['e', 'E']) {
        let mut after = r;
        let negative = after.starts_with('-');
        after = after.strip_prefix(['+', '-']).unwrap_or(after);
        let digits = take_digits(&mut after);
        if !digits.is_empty() {
            let value: i32 = digits.parse().ok()?;
            exponent = Some(if negative { -value } else { value });
            rest = after;
        }
    }
    if !rkm {
        rest = rest.trim_start();
        if rest
            .get(..3)
            .is_some_and(|meg| meg.eq_ignore_ascii_case("meg"))
        {
            prefix = Some('M');
            rest = &rest[3..];
        } else if let Some(letter) = rest.chars().next()
            && PREFIXES.iter().any(|(l, _, _)| *l == letter)
        {
            prefix = Some(letter);
            rest = &rest[letter.len_utf8()..];
        }
    }
    if !rest.is_empty()
        && !units(device)
            .iter()
            .any(|unit| rest == *unit || (*unit == "ohm" && rest.eq_ignore_ascii_case(unit)))
    {
        return None;
    }

    out.push_str(if int.is_empty() { "0" } else { int });
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    let (power, spice) = match prefix {
        Some(letter) => {
            let &(_, power, spice) = PREFIXES.iter().find(|(l, _, _)| *l == letter)?;
            (power, spice)
        }
        None => (0, Some("")),
    };
    match (exponent, spice) {
        (None, Some(spice)) => out.push_str(spice),
        _ => {
            // An exponent that overflows with the prefix is not a number.
            let power = exponent.unwrap_or(0).checked_add(power)?;
            if power != 0 {
                out.push_str(&format!("e{power}"));
            }
        }
    }
    Some(out)
}

/// Converts a parameter value that is a number in KiCad's notation, like [`to_spice`]; anything
/// else (an expression, a name) is returned as is.
pub fn param_to_spice(text: &str) -> String {
    to_spice(text, ' ').unwrap_or_else(|| text.to_string())
}

/// The value of a number in KiCad's notation ([`to_spice`], without a unit), or `None`.
pub fn to_f64(text: &str) -> Option<f64> {
    let spice = to_spice(text, ' ')?;
    let end = spice
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(spice.len());
    let (number, suffix) = spice.split_at(end);
    let power = match suffix {
        "" => 0,
        "Meg" => 6,
        _ if suffix.starts_with('e') => suffix[1..].parse().ok()?,
        _ => {
            let letter = suffix.chars().next()?;
            PREFIXES
                .iter()
                .find(|(l, _, spice)| *l == letter && spice.is_some())?
                .1
        }
    };
    format!("{number}e{power}").parse().ok()
}

fn take_digits<'a>(rest: &mut &'a str) -> &'a str {
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (digits, after) = rest.split_at(end);
    *rest = after;
    digits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_f64() {
        for (text, value) in [
            ("100n", 100e-9),
            ("1M", 1e6),
            ("1Meg", 1e6),
            ("4k7", 4.7e3),
            ("-2.5u", -2.5e-6),
            ("1.5e-9", 1.5e-9),
            ("3a", 3e-18),
            ("50", 50.0),
        ] {
            let got = to_f64(text).unwrap();
            assert!((got - value).abs() <= value.abs() * 1e-12, "{text}: {got}");
        }
        assert_eq!(to_f64("x"), None);
        assert_eq!(to_f64("10V"), None);
    }

    #[test]
    fn test_to_spice() {
        // Compared with kicad-cli 9 (`sch export netlist --format spice`); where the outputs differ,
        // they are the same number for ngspice.
        let cases = [
            ('R', "10k", "10k"),
            ('R', "4k7", "4.7k"),
            ('R', "4K7", "4.7k"),
            ('R', "2R2", "2.2"),
            ('R', "0R", "0"),
            ('R', "1R", "1"),
            ('R', "1k5Ω", "1.5k"),
            ('R', "100Ω", "100"),
            ('R', "1kR", "1k"),
            ('R', "2.2kohm", "2.2k"),
            ('R', "1M", "1Meg"),
            ('R', "1MEG", "1Meg"),
            ('R', "1meg", "1Meg"),
            ('R', "1m", "1m"),
            ('R', "1 k", "1k"),
            ('R', "-5", "-5"),
            ('R', "+3", "3"),
            ('R', "1e3", "1e3"),
            ('R', "1E3", "1e3"),
            ('R', "1T", "1T"),
            ('R', "1P", "1e15"),
            ('C', "1n5", "1.5n"),
            ('C', "4u7F", "4.7u"),
            ('C', "100nF", "100n"),
            ('C', "10mF", "10m"),
            ('C', "1.5e-9", "1.5e-9"),
            ('C', "1e-3u", "1e-9"),
            ('C', ".5u", "0.5u"),
            ('C', "1µ", "1u"),
            ('C', "10f", "10f"),
            ('C', "1F", "1"),
            ('C', "1a", "1e-18"),
            ('L', "10uH", "10u"),
            ('V', "5V", "5"),
            ('I', "1mA", "1m"),
        ];
        for (device, text, expected) in cases {
            assert_eq!(
                to_spice(text, device).as_deref(),
                Some(expected),
                "{device} {text}"
            );
        }
        for (device, text) in [
            ('R', "abc"),
            ('R', "R47"),
            ('R', "1kk"),
            ('R', "1.2.3"),
            ('R', "1u5u"),
            ('R', "5V"),
            ('R', "{x}"),
            ('R', ""),
            ('R', "-"),
            ('R', "10k 1%"),
            ('C', "0R"),
            ('C', "1R5k"),
            // The exponent overflows `i32`, alone or with the prefix.
            ('R', "1e2147483648"),
            ('R', "1e2147483647P"),
            ('C', "1e-2147483648a"),
        ] {
            assert_eq!(to_spice(text, device), None, "{device} {text}");
        }
        assert_eq!(param_to_spice("1M"), "1Meg");
        assert_eq!(param_to_spice("{x*2}"), "{x*2}");
    }
}
