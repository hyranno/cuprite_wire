//! The part standing for a co-simulated device.

use crate::{
    parts::Part,
    spice::{
        SpicePartInfo,
        digital::{
            InputCharacteristics, OutputCharacteristics, SUBCIRCUIT_REFERENCE,
            assert_subcircuit_pin, internal_node, is_identifier, subcircuit_part, subcircuit_text,
        },
    },
};

/// A pin of a [`CosimDevice`]: an output, an input, or both (a bidirectional GPIO).
#[derive(Debug, Clone, PartialEq)]
pub struct CosimPin {
    /// The pin name: a port of the subcircuit, unique (case-insensitively) among the pins of the device.
    pub name: String,
    /// How the pin drives the circuit, if it is an output.
    pub output: Option<OutputCharacteristics>,
    /// How the pin loads and reads the circuit, if it is an input.
    pub input: Option<InputCharacteristics>,
}

/// A pin of a [`CosimDevice`] by its position: what [`CosimDevice::pin_index`] and
/// [`DeviceIo::pin_index`](super::DeviceIo::pin_index) return, to give the pin to the methods of
/// [`DeviceIo`](super::DeviceIo) without looking up its name. It stands for the pin of the device it
/// was taken from; with another device it may name another pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinIndex(pub(super) usize);

/// The position of the pin named `name` (case-insensitive).
pub(super) fn find_pin<'a>(
    pins: impl IntoIterator<Item = &'a CosimPin>,
    name: &str,
) -> Option<usize> {
    pins.into_iter()
        .position(|pin| pin.name.eq_ignore_ascii_case(name))
}

/// A device whose behavior is a Rust [`DeviceModel`](super::DeviceModel), bound by `name` when
/// the deck is run ([`SpiceDeck::run_with`](crate::spice::SpiceDeck::run_with)). Its pins are those
/// added with [`CosimDevice::output`], [`CosimDevice::input`] and [`CosimDevice::bidirectional`],
/// and `gnd`, the reference of their voltages.
///
/// It is written as a subcircuit `cuprite_cosim_<name>` (`XU1 <pins...> gnd cuprite_cosim_<name>`).
/// An output pin `p` is an `EXTERNAL` source `VD_p` (at the node `p#d`) behind the switch `SO_p` of
/// the output resistance, opened to high impedance by the `EXTERNAL` source `VE_p` (at `p#e`); an
/// input pin has the elements of [`DigitalLoad`](crate::spice::digital::DigitalLoad) (`CC_p`,
/// `RP_p`, `II_p`).
///
/// Outputs start `Low`, and bidirectional pins `HiZ`, until the model sets them.
#[derive(Debug, Clone, PartialEq)]
pub struct CosimDevice {
    name: String,
    pins: Vec<CosimPin>,
}

// The values are checked to be finite, so equality is reflexive.
impl Eq for CosimDevice {}

impl CosimDevice {
    /// The reference pin the voltages of all pins are relative to.
    pub const GND: &'static str = "gnd";

    /// # Panics
    /// If `name` is not made of ASCII letters, digits and `_`.
    #[track_caller]
    pub fn new(name: &str) -> Box<Self> {
        assert!(
            is_identifier(name),
            "the name of a co-simulated device may only contain ASCII letters, digits and '_': {name:?}"
        );
        Box::new(CosimDevice {
            name: name.to_string(),
            pins: Vec::new(),
        })
    }

    /// Adds an output pin.
    ///
    /// # Panics
    /// If the name is not made of ASCII letters, digits and `_`, is `gnd`, `ref`, `0` or another
    /// pin's name (case-insensitively), or the characteristics are invalid.
    #[track_caller]
    pub fn output(self: Box<Self>, pin: &str, output: OutputCharacteristics) -> Box<Self> {
        self.pin(pin, Some(output), None)
    }

    /// Adds an input pin (digital or analog: the model reads its voltage too).
    ///
    /// # Panics
    /// As [`CosimDevice::output`].
    #[track_caller]
    pub fn input(self: Box<Self>, pin: &str, input: InputCharacteristics) -> Box<Self> {
        self.pin(pin, None, Some(input))
    }

    /// Adds a pin that is both an output and an input.
    ///
    /// # Panics
    /// As [`CosimDevice::output`].
    #[track_caller]
    pub fn bidirectional(
        self: Box<Self>,
        pin: &str,
        output: OutputCharacteristics,
        input: InputCharacteristics,
    ) -> Box<Self> {
        self.pin(pin, Some(output), Some(input))
    }

    #[track_caller]
    fn pin(
        mut self: Box<Self>,
        name: &str,
        output: Option<OutputCharacteristics>,
        input: Option<InputCharacteristics>,
    ) -> Box<Self> {
        assert_subcircuit_pin(
            name,
            self.pins.iter().map(|pin| pin.name.as_str()),
            &format!("the device {:?}", self.name),
        );
        if let Some(output) = &output {
            output.validate();
        }
        if let Some(input) = &input {
            input.validate();
        }
        self.pins.push(CosimPin {
            name: name.to_string(),
            output,
            input,
        });
        self
    }

    /// The name models are bound by.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The pins in the order they were added (the order of [`PinIndex`]), without `gnd`.
    pub fn pins(&self) -> &[CosimPin] {
        &self.pins
    }

    /// The index of the pin named `name` (case-insensitive), to look it up once before the run.
    ///
    /// # Panics
    /// If the device has no such pin (`gnd` is not one).
    #[track_caller]
    pub fn pin_index(&self, name: &str) -> PinIndex {
        PinIndex(
            find_pin(&self.pins, name)
                .unwrap_or_else(|| panic!("the device {:?} has no pin {name:?}", self.name)),
        )
    }

    fn subcircuit_name(&self) -> String {
        format!("cuprite_cosim_{}", self.name)
    }

    fn subcircuit(&self) -> String {
        let ports: Vec<&str> = self.pins.iter().map(|p| p.name.as_str()).collect();
        let mut body = String::new();
        for pin in &self.pins {
            let p = &pin.name;
            if let Some(output) = &pin.output {
                let (drive, enable) = (internal_node(p, "d"), internal_node(p, "e"));
                body.push_str(&format!(
                    "VD_{p} {drive} {SUBCIRCUIT_REFERENCE} DC 0 EXTERNAL\n\
                     VE_{p} {enable} {SUBCIRCUIT_REFERENCE} DC 0 EXTERNAL\n"
                ));
                body.push_str(&output.switch_lines(&format!("_{p}"), &drive, p, &enable));
            }
            if let Some(input) = &pin.input {
                body.push_str(&input.load_lines(p));
            }
        }
        subcircuit_text(&self.subcircuit_name(), &ports, &body)
    }

    /// The name of the `EXTERNAL` source of a pin (`VD_p` drive, `VE_p` enable) as the callbacks
    /// of ngspice give it for the element `element` (`XU1`): `v.xu1.vd_p`.
    pub(crate) fn source_name(element: &str, kind: char, pin: &str) -> String {
        format!("v.{element}.v{kind}_{pin}").to_ascii_lowercase()
    }
}

impl Part for CosimDevice {
    fn get_ref_base_name(&self) -> String {
        "U".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        self.pins
            .iter()
            .map(|p| p.name.clone())
            .chain([Self::GND.to_string()])
            .collect()
    }
    fn value(&self) -> String {
        self.name.clone()
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(subcircuit_part(
            self.pin_ids(),
            self.subcircuit_name(),
            self.subcircuit(),
        ))
    }
    fn cosim_device(&self) -> Option<&CosimDevice> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subcircuit() {
        let device = CosimDevice::new("mcu")
            .output("pa0", OutputCharacteristics::cmos(3.3))
            .input(
                "adc",
                InputCharacteristics::cmos(3.3).with_capacitance(10e-12),
            );
        assert_eq!(device.pin_ids(), ["pa0", "adc", "gnd"]);
        assert_eq!(
            device.subcircuit(),
            ".subckt cuprite_cosim_mcu pa0 adc ref\n\
             VD_pa0 pa0#d ref DC 0 EXTERNAL\n\
             VE_pa0 pa0#e ref DC 0 EXTERNAL\n\
             SO_pa0 pa0#d pa0 pa0#e ref hiz_pa0\n\
             .model hiz_pa0 SW( vt=0.5 vh=0 ron=50 roff=1e9 )\n\
             CC_adc adc ref 1e-11\n\
             .ends cuprite_cosim_mcu\n"
        );
        assert_eq!(CosimDevice::source_name("XU1", 'd', "PA0"), "v.xu1.vd_pa0");
    }

    #[test]
    #[should_panic(expected = "already has a pin \"GND\"")]
    fn test_pin_named_gnd() {
        let _ = CosimDevice::new("mcu").input("GND", InputCharacteristics::cmos(3.3));
    }

    /// The node of the pin `d_p` is not the drive node of the output `p` (both were `d_p`).
    #[test]
    fn test_internal_nodes_apart_from_pins() {
        let device = CosimDevice::new("mcu")
            .output("p", OutputCharacteristics::cmos(3.3))
            .input("d_p", InputCharacteristics::cmos(3.3));
        let text = device.subcircuit();
        assert!(text.contains("VD_p p#d ref DC 0 EXTERNAL\n"), "{text}");
        assert!(text.contains("CC_d_p d_p ref 5e-12\n"), "{text}");
    }

    #[test]
    #[should_panic(expected = "the pin name \"0\" is reserved")]
    fn test_pin_named_0() {
        let _ = CosimDevice::new("mcu").output("0", OutputCharacteristics::cmos(3.3));
    }

    #[test]
    #[should_panic(expected = "may only contain ASCII letters")]
    fn test_invalid_name() {
        let _ = CosimDevice::new("my mcu");
    }
}
