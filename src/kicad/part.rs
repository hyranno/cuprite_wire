use std::{collections::BTreeMap, fmt, str::FromStr};

/// A KiCad library identifier, `Library:Name` (e.g. `Resistor_SMD:R_0603_1608Metric`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LibId {
    library: String,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibIdError {
    pub lib_id: String,
    pub reason: &'static str,
}

impl fmt::Display for LibIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid library identifier {:?}: {}", self.lib_id, self.reason)
    }
}

impl std::error::Error for LibIdError {}

impl LibId {
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

    pub fn library(&self) -> &str {
        &self.library
    }

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

/// Electrical type of a pin (`pintype` in the netlist).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PinType {
    Input,
    Output,
    Bidirectional,
    TriState,
    #[default]
    Passive,
    Free,
    Unspecified,
    PowerIn,
    PowerOut,
    OpenCollector,
    OpenEmitter,
    NoConnect,
}

impl PinType {
    pub fn as_str(self) -> &'static str {
        match self {
            PinType::Input => "input",
            PinType::Output => "output",
            PinType::Bidirectional => "bidirectional",
            PinType::TriState => "tri_state",
            PinType::Passive => "passive",
            PinType::Free => "free",
            PinType::Unspecified => "unspecified",
            PinType::PowerIn => "power_in",
            PinType::PowerOut => "power_out",
            PinType::OpenCollector => "open_collector",
            PinType::OpenEmitter => "open_emitter",
            PinType::NoConnect => "no_connect",
        }
    }
}

/// KiCad information of a pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KicadPinInfo {
    /// Footprint pad number (`"1"`, `"A3"`, ...).
    pub pad: String,
    /// Pin name, written as `pinfunction`.
    pub name: Option<String>,
    pub pin_type: PinType,
}

impl KicadPinInfo {
    pub fn new(pad: impl Into<String>, name: Option<&str>, pin_type: PinType) -> Self {
        KicadPinInfo {
            pad: pad.into(),
            name: name.map(str::to_string),
            pin_type,
        }
    }
}

/// Component flags written as value-less properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PartFlags {
    pub dnp: bool,
    pub exclude_from_bom: bool,
    pub exclude_from_board: bool,
}

/// KiCad-specific information of a part.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KicadPartInfo {
    /// The footprint. Exporting a part without one is an error.
    pub footprint: Option<LibId>,
    /// Pin information keyed by pin ID. Every pin ID of the part needs exactly one entry.
    pub pins: BTreeMap<String, KicadPinInfo>,
    /// The schematic symbol the part corresponds to, if any.
    pub lib_source: Option<LibId>,
    /// Additional fields imported into the footprint.
    pub fields: Vec<(String, String)>,
    pub flags: PartFlags,
}

#[cfg(test)]
mod tests {
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
            assert!(invalid.parse::<LibId>().is_err(), "{invalid:?} should be rejected");
        }
    }
}
