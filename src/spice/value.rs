//! Numbers and values as written in SPICE netlists.

/// Formats a number for SPICE: plain decimal for moderate magnitudes, exponent form otherwise.
///
/// # Panics
/// If `value` is NaN or infinite, which ngspice cannot read.
#[track_caller]
pub fn spice_number(value: f64) -> String {
    assert!(value.is_finite(), "a SPICE number must be finite: {value}");
    let magnitude = value.abs();
    if value == 0.0 || (1e-3..1e6).contains(&magnitude) {
        format!("{value}")
    } else {
        format!("{value:e}")
    }
}

/// Converts a value in KiCad's notation (SI prefixes with `M` for mega, the RKM code like `4k7`,
/// an optional unit like `100nF`) to SPICE (`4.7k`, `1Meg`, `100n`), for an element of `device`
/// (`R`, `C`, `L`, `V`, `I`), as KiCad does when it writes a SPICE netlist. Parts made by
/// [`kicad_part!`](crate::kicad_part) with `value: param` write their value with it.
///
/// # Errors
///
/// If the value is not such a number.
pub fn spice_value(device: char, value: &str) -> Result<String, String> {
    cuprite_wire_kicad_lib::value::try_to_spice(value, device).map_err(|e| e.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spice_number() {
        assert_eq!(spice_number(0.0), "0");
        assert_eq!(spice_number(5.0), "5");
        assert_eq!(spice_number(-1.5), "-1.5");
        assert_eq!(spice_number(0.001), "0.001");
        assert_eq!(spice_number(1e-9), "1e-9");
        assert_eq!(spice_number(2.5e6), "2.5e6");
    }

    #[test]
    #[should_panic(expected = "a SPICE number must be finite: NaN")]
    fn test_spice_number_nan() {
        spice_number(f64::NAN);
    }

    #[test]
    fn test_spice_value() {
        assert_eq!(spice_value('R', "4k7").unwrap(), "4.7k");
        assert_eq!(spice_value('C', "100nF").unwrap(), "100n");
        assert!(spice_value('R', "abc").is_err());
    }
}
