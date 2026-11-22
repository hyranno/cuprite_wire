//! Helpers for the parts written as generated subcircuits.

use crate::spice::{GROUND, ModelSource, SpiceElement, SpicePartInfo};

/// The node of the generated subcircuits that their `gnd` pin connects to. ngspice reads a node
/// named `gnd` as the ground (node 0), even as a port of a subcircuit, so it cannot be used there.
pub(crate) const SUBCIRCUIT_REFERENCE: &str = "ref";

/// Names written into generated subcircuits: ASCII letters, digits and `_`.
pub(crate) fn is_identifier(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The node of a generated subcircuit that serves `role` for the pin `pin`: `<pin>#<role>`.
/// Pin names cannot contain `#` ([`check_subcircuit_pin`]), so it is never a pin's node.
pub(crate) fn internal_node(pin: &str, role: &str) -> String {
    format!("{pin}#{role}")
}

/// Checks the name of a pin added to a generated subcircuit, of a part that has the pins
/// `existing` and `gnd`: an identifier ([`is_identifier`]), none of the part's pin names
/// (case-insensitively, as ngspice reads them), and not [`SUBCIRCUIT_REFERENCE`] or [`GROUND`],
/// which name other nodes of the subcircuit. `owner` names the part in the message.
pub(crate) fn check_subcircuit_pin<'a>(
    pin: &str,
    existing: impl IntoIterator<Item = &'a str>,
    owner: &str,
) -> Result<(), String> {
    if !is_identifier(pin) {
        Err(format!(
            "a pin name may only contain ASCII letters, digits and '_': {pin:?}"
        ))
    } else if [SUBCIRCUIT_REFERENCE, GROUND]
        .iter()
        .any(|node| pin.eq_ignore_ascii_case(node))
    {
        Err(format!(
            "the pin name {pin:?} is reserved for a node of the subcircuit"
        ))
    } else if existing
        .into_iter()
        .chain(["gnd"])
        .any(|name| name.eq_ignore_ascii_case(pin))
    {
        Err(format!("{owner} already has a pin {pin:?}"))
    } else {
        Ok(())
    }
}

/// [`check_subcircuit_pin`], panicking on an invalid name.
#[track_caller]
pub(crate) fn assert_subcircuit_pin<'a>(
    pin: &str,
    existing: impl IntoIterator<Item = &'a str>,
    owner: &str,
) {
    if let Err(message) = check_subcircuit_pin(pin, existing, owner) {
        panic!("{message}");
    }
}

/// The definition of the subcircuit `name` with the ports `ports` and [`SUBCIRCUIT_REFERENCE`].
pub(crate) fn subcircuit_text(name: &str, ports: &[&str], body: &str) -> String {
    let mut header = format!(".subckt {name}");
    for port in ports {
        header.push(' ');
        header.push_str(port);
    }
    format!("{header} {SUBCIRCUIT_REFERENCE}\n{body}.ends {name}\n")
}

/// The name `cuprite_<prefix>_<hash>` and the definition ([`subcircuit_text`]) of a subcircuit.
/// The name is derived from the definition, so equal parts share one definition.
pub(crate) fn hashed_subcircuit(prefix: &str, ports: &[&str], body: &str) -> (String, String) {
    let hash = fnv1a(subcircuit_text("", ports, body).as_bytes());
    let name = format!("cuprite_{prefix}_{hash:016x}");
    let text = subcircuit_text(&name, ports, body);
    (name, text)
}

/// The SPICE information of a part written as the subcircuit `name` defined by `text`, with the
/// part's pins `pins` as its nodes.
pub(crate) fn subcircuit_part<S: Into<String>>(
    pins: impl IntoIterator<Item = S>,
    name: String,
    text: String,
) -> SpicePartInfo {
    SpicePartInfo::element(
        SpiceElement::new('X', pins, name.clone()).with_model(ModelSource::inline(name, text)),
    )
}

/// The 64-bit FNV-1a hash: stable across platforms and Rust versions, unlike `DefaultHasher`.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_subcircuit_pin() {
        let check = |pin| check_subcircuit_pin(pin, ["a", "B"], "the part");
        assert_eq!(check("c_1"), Ok(()));
        for (pin, message) in [
            ("b", "the part already has a pin \"b\""),
            ("GND", "the part already has a pin \"GND\""),
            (
                "Ref",
                "the pin name \"Ref\" is reserved for a node of the subcircuit",
            ),
            (
                "0",
                "the pin name \"0\" is reserved for a node of the subcircuit",
            ),
            (
                "a#ai",
                "a pin name may only contain ASCII letters, digits and '_': \"a#ai\"",
            ),
            (
                "",
                "a pin name may only contain ASCII letters, digits and '_': \"\"",
            ),
        ] {
            assert_eq!(check(pin), Err(message.to_string()));
        }
    }

    #[test]
    fn test_hashed_subcircuit() {
        let (name, text) = hashed_subcircuit("x", &["a", "b"], "R1 a b 1k\n");
        assert!(name.starts_with("cuprite_x_"), "{name}");
        assert_eq!(
            text,
            format!(".subckt {name} a b ref\nR1 a b 1k\n.ends {name}\n")
        );
        // The ports are part of the definition.
        assert_ne!(hashed_subcircuit("x", &["b", "a"], "R1 a b 1k\n").0, name);
    }
}
