//! Numbers as written in IBIS files.

/// Parses a number: a decimal number with an optional exponent, an optional scale suffix
/// (`T` `G` `M` `k` `m` `u` `n` `p` `f`, case-sensitive) and ignored trailing letters, the units
/// (`1.0pF`, `5.0V`, `55mOhm`). `None` if it is not a number or is out of the range of `f64`
/// (`1e999`).
pub(crate) fn parse_number(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    let digits = |mut i: usize| {
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        i
    };
    let mut i = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let int_end = digits(i);
    let mut has_digits = int_end > i;
    i = int_end;
    if bytes.get(i) == Some(&b'.') {
        let frac_end = digits(i + 1);
        has_digits |= frac_end > i + 1;
        i = frac_end;
    }
    if !has_digits {
        return None;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(bytes.get(i + 1), Some(b'+' | b'-')));
        let exp_end = digits(i + 1 + sign);
        if exp_end > i + 1 + sign {
            i = exp_end;
        }
    }
    let mantissa: f64 = s[..i].parse().ok()?;
    let scale = match bytes.get(i) {
        Some(b'T') => 1e12,
        Some(b'G') => 1e9,
        Some(b'M') => 1e6,
        Some(b'k') => 1e3,
        Some(b'm') => 1e-3,
        Some(b'u') => 1e-6,
        Some(b'n') => 1e-9,
        Some(b'p') => 1e-12,
        Some(b'f') => 1e-15,
        _ => 1.0,
    };
    let value = mantissa * scale;
    (value.is_finite() && s[i..].bytes().all(|b| b.is_ascii_alphabetic())).then_some(value)
}

/// Parses a number or `NA` (not available, `Some(None)`). `None` if it is neither.
pub(crate) fn parse_optional(s: &str) -> Option<Option<f64>> {
    if s.eq_ignore_ascii_case("NA") {
        Some(None)
    } else {
        parse_number(s).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(s: &str, expected: f64) {
        let v = parse_number(s).unwrap_or_else(|| panic!("{s} is not a number"));
        assert!(
            (v - expected).abs() <= expected.abs() * 1e-12,
            "{s}: {v} != {expected}"
        );
    }

    #[test]
    fn test_parse_number() {
        approx("0", 0.0);
        approx("5.0V", 5.0);
        approx("-50.0m", -0.05);
        approx("1.0pF", 1e-12);
        approx("5mm", 5e-3);
        approx("2n", 2e-9);
        approx("+.5", 0.5);
        approx("3.", 3.0);
        approx("1e-3", 1e-3);
        approx("1.5E+2A", 150.0);
        approx("2MOhm", 2e6);
        approx("4k", 4e3);
        approx("7T", 7e12);
        approx("1G", 1e9);
        approx("3u", 3e-6);
        approx("6f", 6e-15);
        // An `e` without digits is a unit, not an exponent.
        approx("1e", 1.0);
        for s in [
            "", "-", ".", "V", "NA", "1.2.3", "3.0/30n", "1-2", "1e999", "1e300T",
        ] {
            assert_eq!(parse_number(s), None, "{s}");
        }
    }

    #[test]
    fn test_parse_optional() {
        assert_eq!(parse_optional("NA"), Some(None));
        assert_eq!(parse_optional("na"), Some(None));
        assert_eq!(parse_optional("2"), Some(Some(2.0)));
        assert_eq!(parse_optional("x"), None);
    }
}
