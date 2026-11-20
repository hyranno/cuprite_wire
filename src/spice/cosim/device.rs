//! The part standing for a co-simulated device.

use crate::{
    parts::Part,
    spice::{
        ModelSource, SpiceElement, SpicePartInfo,
        digital::{InputCharacteristics, OutputCharacteristics},
    },
};

/// A pin of a [`CosimDevice`]: an output, an input, or both (a bidirectional GPIO).
#[derive(Debug, Clone, PartialEq)]
pub struct CosimPin {
    pub name: String,
    pub output: Option<OutputCharacteristics>,
    pub input: Option<InputCharacteristics>,
}

/// A device whose behavior is a Rust [`DeviceModel`](super::DeviceModel), bound by `name` when
/// the deck is run ([`SpiceDeck::run_with`](crate::spice::SpiceDeck::run_with)). Its pins are those
/// added with [`CosimDevice::output`], [`CosimDevice::input`] and [`CosimDevice::bidirectional`],
/// and `gnd`, the reference of their voltages.
///
/// It is written as a subcircuit `cuprite_cosim_<name>` (`XU1 <pins...> gnd cuprite_cosim_<name>`).
/// An output pin `p` is an `EXTERNAL` source `VD_p` behind the switch `SO_p` of the output
/// resistance, opened to high impedance by the `EXTERNAL` source `VE_p`; an input pin has the
/// elements of [`DigitalLoad`](crate::spice::digital::DigitalLoad) (`CC_p`, `RP_p`, `II_p`).
///
/// Outputs start `Low`, and bidirectional pins `HiZ`, until the model sets them.
#[derive(Debug, Clone, PartialEq)]
pub struct CosimDevice {
    name: String,
    pins: Vec<CosimPin>,
}

// The values are checked to be finite, so equality is reflexive.
impl Eq for CosimDevice {}

/// Names written into the subcircuit: ASCII letters, digits and `_`.
fn is_identifier(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl CosimDevice {
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
    /// If the name is not made of ASCII letters, digits and `_`, is `gnd` or another pin's name
    /// (case-insensitively), or the characteristics are invalid.
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
        assert!(
            is_identifier(name),
            "a pin name may only contain ASCII letters, digits and '_': {name:?}"
        );
        assert!(
            !name.eq_ignore_ascii_case(Self::GND)
                && !self.pins.iter().any(|p| p.name.eq_ignore_ascii_case(name)),
            "the device {:?} already has a pin {name:?}",
            self.name
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

    pub fn pins(&self) -> &[CosimPin] {
        &self.pins
    }

    fn subcircuit_name(&self) -> String {
        format!("cuprite_cosim_{}", self.name)
    }

    fn subcircuit(&self) -> String {
        let name = self.subcircuit_name();
        let ports: Vec<&str> = self.pins.iter().map(|p| p.name.as_str()).collect();
        let mut text = format!(".subckt {name} {} {}\n", ports.join(" "), Self::GND);
        for pin in &self.pins {
            let p = &pin.name;
            if let Some(output) = &pin.output {
                text.push_str(&format!("VD_{p} d_{p} gnd DC 0 EXTERNAL\n"));
                text.push_str(&format!("VE_{p} e_{p} gnd DC 0 EXTERNAL\n"));
                text.push_str(&output.switch_lines(
                    &format!("_{p}"),
                    &format!("d_{p}"),
                    p,
                    &format!("e_{p}"),
                ));
            }
            if let Some(input) = &pin.input {
                for element in input.load_elements(p, Self::GND) {
                    text.push_str(&format!(
                        "{}{}_{p} {} {}\n",
                        element.device,
                        element.suffix,
                        element.nodes.join(" "),
                        element.args
                    ));
                }
            }
        }
        text.push_str(&format!(".ends {name}\n"));
        text
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
        let name = self.subcircuit_name();
        Some(SpicePartInfo::element(
            SpiceElement::new('X', self.pin_ids(), name.clone())
                .with_model(ModelSource::inline(name, self.subcircuit())),
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
            ".subckt cuprite_cosim_mcu pa0 adc gnd\n\
             VD_pa0 d_pa0 gnd DC 0 EXTERNAL\n\
             VE_pa0 e_pa0 gnd DC 0 EXTERNAL\n\
             SO_pa0 d_pa0 pa0 e_pa0 gnd hiz_pa0\n\
             .model hiz_pa0 SW( vt=0.5 vh=0 ron=50 roff=1e9 )\n\
             CC_adc adc gnd 1e-11\n\
             .ends cuprite_cosim_mcu\n"
        );
        assert_eq!(CosimDevice::source_name("XU1", 'd', "PA0"), "v.xu1.vd_pa0");
    }

    #[test]
    #[should_panic(expected = "already has a pin \"GND\"")]
    fn test_pin_named_gnd() {
        let _ = CosimDevice::new("mcu").input("GND", InputCharacteristics::cmos(3.3));
    }

    #[test]
    #[should_panic(expected = "may only contain ASCII letters")]
    fn test_invalid_name() {
        let _ = CosimDevice::new("my mcu");
    }
}
