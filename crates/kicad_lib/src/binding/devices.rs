//! The devices of the `.model` types.

use crate::spice_lib::strip_comment;

/// A device with a `.model`: its KiCad `Sim.Device`, element letter and terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelDevice {
    pub sim_device: &'static str,
    pub letter: char,
    pub terminals: &'static [&'static str],
}

const DIODE: &[&str] = &["A", "K"];
const BJT: &[&str] = &["C", "B", "E"];
const MOSFET: &[&str] = &["D", "G", "S", "B"];
const THREE_TERMINAL_FET: &[&str] = &["D", "G", "S"];

/// `.model` types (lowercase), with their devices.
const MODEL_DEVICES: &[(&str, ModelDevice)] = &[
    ("d", device("D", 'D', DIODE)),
    ("npn", device("NPN", 'Q', BJT)),
    ("pnp", device("PNP", 'Q', BJT)),
    ("nmos", device("NMOS", 'M', MOSFET)),
    ("pmos", device("PMOS", 'M', MOSFET)),
    // N-channel unless the parameters have `pchan`.
    ("vdmos", device("NMOS", 'M', THREE_TERMINAL_FET)),
    ("njf", device("NJFET", 'J', THREE_TERMINAL_FET)),
    ("pjf", device("PJFET", 'J', THREE_TERMINAL_FET)),
];

const fn device(
    sim_device: &'static str,
    letter: char,
    terminals: &'static [&'static str],
) -> ModelDevice {
    ModelDevice {
        sim_device,
        letter,
        terminals,
    }
}

/// The device of a `.model` of `model_type` (lowercase) defined by `text` (the `.model`
/// statement, which tells the channel of a VDMOS).
pub fn model_device(model_type: &str, text: &str) -> Option<ModelDevice> {
    let (_, mut device) = *MODEL_DEVICES.iter().find(|(t, _)| *t == model_type)?;
    if model_type == "vdmos" && has_pchan(text) {
        device.sim_device = "PMOS";
    }
    Some(device)
}

/// Whether the parameters of a `.model` have the `pchan` flag.
fn has_pchan(text: &str) -> bool {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('*'))
        .map(strip_comment)
        .flat_map(|line| line.split(|c: char| c.is_whitespace() || "(),=".contains(c)))
        .filter(|word| !word.is_empty())
        .skip(3)
        .any(|word| word.eq_ignore_ascii_case("pchan"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_devices() {
        let device = |text: &str| {
            let model_type = text.split([' ', '(']).nth(2).unwrap().to_ascii_lowercase();
            model_device(&model_type, text).map(|d| (d.sim_device, d.letter, d.terminals.len()))
        };
        assert_eq!(device(".model a D"), Some(("D", 'D', 2)));
        assert_eq!(device(".model a NMOS level=1"), Some(("NMOS", 'M', 4)));
        assert_eq!(device(".model a VDMOS(Kp=1)"), Some(("NMOS", 'M', 3)));
        assert_eq!(
            device(".model a VDMOS(Kp=1\n+ PCHAN)"),
            Some(("PMOS", 'M', 3))
        );
        assert_eq!(device(".model pchan VDMOS NCHAN"), Some(("NMOS", 'M', 3)));
        assert_eq!(device(".model a VDMOS PCHAN"), Some(("PMOS", 'M', 3)));
        // Inline comments are not parameters.
        assert_eq!(
            device(".model a VDMOS(Kp=1) $ pchan"),
            Some(("NMOS", 'M', 3))
        );
        assert_eq!(
            device(".model a VDMOS(Kp=1) // pchan"),
            Some(("NMOS", 'M', 3))
        );
        assert_eq!(device(".model a SW"), None);
    }
}
