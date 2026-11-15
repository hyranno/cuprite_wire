//! Parts shared by the unit tests.

use std::collections::BTreeMap;

use crate::{
    kicad::{KicadPartInfo, KicadPinInfo, LibId, PinType},
    parts::Part,
    spice::SpicePartInfo,
};

fn lib_id(s: &str) -> LibId {
    s.parse().unwrap()
}

fn two_pin_kicad_info(footprint: &str, symbol: &str) -> KicadPartInfo {
    KicadPartInfo {
        footprint: Some(lib_id(footprint)),
        pins: BTreeMap::from([
            ("1".to_string(), KicadPinInfo::new("1", None, PinType::Passive)),
            ("2".to_string(), KicadPinInfo::new("2", None, PinType::Passive)),
        ]),
        lib_source: Some(lib_id(symbol)),
        ..Default::default()
    }
}

#[derive(Debug, Clone)]
pub struct Resistor(pub &'static str);

impl Part for Resistor {
    fn get_ref_base_name(&self) -> String {
        "R".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec!["1".into(), "2".into()]
    }
    fn value(&self) -> String {
        self.0.into()
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        Some(two_pin_kicad_info("Resistor_SMD:R_0603_1608Metric", "Device:R"))
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(SpicePartInfo {})
    }
}

#[derive(Debug, Clone)]
pub struct Capacitor(pub &'static str);

impl Part for Capacitor {
    fn get_ref_base_name(&self) -> String {
        "C".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec!["1".into(), "2".into()]
    }
    fn value(&self) -> String {
        self.0.into()
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        Some(two_pin_kicad_info("Capacitor_SMD:C_0603_1608Metric", "Device:C"))
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(SpicePartInfo {})
    }
}

/// A single op-amp in a 5-pin package.
#[derive(Debug, Clone)]
pub struct OpAmp;

impl Part for OpAmp {
    fn get_ref_base_name(&self) -> String {
        "U".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        ["out", "vss", "in_p", "in_n", "vdd"].map(String::from).to_vec()
    }
    fn value(&self) -> String {
        "OpAmp".into()
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        let pins = [
            ("out", "1", "OUT", PinType::Output),
            ("vss", "2", "V-", PinType::PowerIn),
            ("in_p", "3", "+", PinType::Input),
            ("in_n", "4", "-", PinType::Input),
            ("vdd", "5", "V+", PinType::PowerIn),
        ];
        Some(KicadPartInfo {
            footprint: Some(lib_id("Package_TO_SOT_SMD:SOT-23-5")),
            pins: pins
                .into_iter()
                .map(|(id, pad, name, pin_type)| (id.to_string(), KicadPinInfo::new(pad, Some(name), pin_type)))
                .collect(),
            ..Default::default()
        })
    }
}

/// An IC with three GND pads connected inside, and a pad with a name containing `/`.
#[derive(Debug, Clone)]
pub struct MultiGndIc;

impl Part for MultiGndIc {
    fn get_ref_base_name(&self) -> String {
        "U".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        ["vcc", "gnd_a", "gnd_b", "gnd_c", "io"].map(String::from).to_vec()
    }
    fn value(&self) -> String {
        "MultiGnd".into()
    }
    fn internal_connections(&self) -> Vec<Vec<String>> {
        vec![["gnd_a", "gnd_b", "gnd_c"].map(String::from).to_vec()]
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        let pins = [
            ("vcc", "1", "VCC", PinType::PowerIn),
            ("gnd_a", "2", "GND", PinType::PowerIn),
            ("gnd_b", "10", "GND", PinType::Passive),
            ("gnd_c", "11", "GND", PinType::Passive),
            ("io", "3", "I/O", PinType::Bidirectional),
        ];
        Some(KicadPartInfo {
            footprint: Some(lib_id("Package_SO:SOIC-8_3.9x4.9mm_P1.27mm")),
            pins: pins
                .into_iter()
                .map(|(id, pad, name, pin_type)| (id.to_string(), KicadPinInfo::new(pad, Some(name), pin_type)))
                .collect(),
            ..Default::default()
        })
    }
}

/// A simulation-only voltage source without KiCad information.
#[derive(Debug, Clone)]
pub struct SpiceSource;

impl Part for SpiceSource {
    fn get_ref_base_name(&self) -> String {
        "V".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec!["p".into(), "n".into()]
    }
    fn value(&self) -> String {
        "DC 5".into()
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(SpicePartInfo {})
    }
}

/// A part whose information is given directly, for testing invalid definitions.
#[derive(Debug, Clone)]
pub struct CustomPart {
    pub pin_ids: Vec<&'static str>,
    pub internal_connections: Vec<Vec<&'static str>>,
    pub kicad: Option<KicadPartInfo>,
}

impl CustomPart {
    /// A two-pin part with valid KiCad information.
    pub fn valid() -> Self {
        CustomPart {
            pin_ids: vec!["1", "2"],
            internal_connections: vec![],
            kicad: Some(two_pin_kicad_info("Custom:Footprint", "Custom:Symbol")),
        }
    }
}

impl Part for CustomPart {
    fn get_ref_base_name(&self) -> String {
        "X".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        self.pin_ids.iter().map(|s| s.to_string()).collect()
    }
    fn value(&self) -> String {
        "custom".into()
    }
    fn internal_connections(&self) -> Vec<Vec<String>> {
        self.internal_connections
            .iter()
            .map(|group| group.iter().map(|s| s.to_string()).collect())
            .collect()
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        self.kicad.clone()
    }
}
