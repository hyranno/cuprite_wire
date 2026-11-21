//! SPICE-specific part information.
//!
//! The counterpart of KiCad's `Sim.Device` / `Sim.Params` / `Sim.Library` / `Sim.Name` /
//! `Sim.Pins` fields and the `exclude_from_sim` flag (see [SPICE Netlist Format](https://github.com/hyranno/cuprite_wire/blob/main/doc/dev/spice_netlist_format.md)).

use std::{
    collections::{BTreeSet, HashSet},
    path::PathBuf,
};

/// How a part is simulated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpicePartInfo {
    /// The part is written as one or more SPICE elements.
    Elements {
        elements: Vec<SpiceElement>,
        /// Pins intentionally left out of the model (e.g. NC pins). Every other pin must be
        /// used by an element, or be connected inside the part to a pin that is.
        unused_pins: Vec<String>,
    },
    /// The part is left out of the simulation (connectors, mounting holes, test points, ...).
    /// Its pins still take part in naming the nets.
    Excluded,
}

impl SpicePartInfo {
    /// A part written as a single element.
    pub fn element(element: SpiceElement) -> Self {
        SpicePartInfo::elements([element])
    }

    /// A part written as several elements (e.g. the units of a dual op-amp).
    /// Give each a distinct suffix with [`SpiceElement::with_suffix`].
    pub fn elements(elements: impl IntoIterator<Item = SpiceElement>) -> Self {
        SpicePartInfo::Elements {
            elements: elements.into_iter().collect(),
            unused_pins: Vec::new(),
        }
    }

    /// Declares pins that the model intentionally does not use.
    /// Has no effect on [`SpicePartInfo::Excluded`].
    pub fn with_unused_pins<S: Into<String>>(mut self, pins: impl IntoIterator<Item = S>) -> Self {
        if let SpicePartInfo::Elements { unused_pins, .. } = &mut self {
            unused_pins.extend(pins.into_iter().map(Into::into));
        }
        self
    }

    /// Checks the information against the pins of the part.
    pub fn check(
        &self,
        pin_ids: &[String],
        internal_connections: &[Vec<String>],
    ) -> Vec<SpiceProblem> {
        let SpicePartInfo::Elements {
            elements,
            unused_pins,
        } = self
        else {
            return Vec::new();
        };
        let mut problems = Vec::new();
        if elements.is_empty() {
            problems.push(SpiceProblem::NoElements);
        }
        let pin_id_set: HashSet<&String> = pin_ids.iter().collect();
        let mut suffixes = HashSet::new();
        for element in elements {
            problems.extend(element.check());
            if !suffixes.insert(element.suffix.to_ascii_uppercase()) {
                problems.push(SpiceProblem::DuplicateSuffix {
                    suffix: element.suffix.clone(),
                });
            }
        }
        let used: BTreeSet<&String> = elements
            .iter()
            .flat_map(|element| &element.nodes)
            .chain(unused_pins)
            .collect();
        for pin_id in &used {
            if !pin_id_set.contains(pin_id) {
                problems.push(SpiceProblem::UnknownPin {
                    pin_id: pin_id.to_string(),
                });
            }
        }
        let connected_to_used = |pin_id: &String| {
            internal_connections.iter().any(|group| {
                group.contains(pin_id) && group.iter().any(|other| used.contains(other))
            })
        };
        for pin_id in pin_ids {
            if !used.contains(pin_id) && !connected_to_used(pin_id) {
                problems.push(SpiceProblem::UnmappedPin {
                    pin_id: pin_id.clone(),
                });
            }
        }
        problems
    }
}

/// One SPICE element line: `<device><ref><suffix> <nodes...> <args>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpiceElement {
    /// Appended to the element name. Empty for a part with a single element,
    /// `A`, `B`, ... for the units of a multi-unit part.
    pub suffix: String,
    /// The SPICE device letter (`R`, `C`, `V`, `D`, `Q`, `M`, `X`, ...). It selects the element type,
    /// so it must match the model, whatever the reference designator of the part is.
    pub device: char,
    /// Pin IDs of the part in the order of the model's nodes (the counterpart of `Sim.Pins`).
    pub nodes: Vec<String>,
    /// Written after the nodes: the value, model or subcircuit name and parameters
    /// (e.g. `10k`, `DC 5 AC 1`, `1N4148`, `AD8051`).
    pub args: String,
    /// Model definitions the element needs.
    pub models: Vec<ModelSource>,
}

impl SpiceElement {
    pub fn new<S: Into<String>>(
        device: char,
        nodes: impl IntoIterator<Item = S>,
        args: impl Into<String>,
    ) -> Self {
        SpiceElement {
            suffix: String::new(),
            device,
            nodes: nodes.into_iter().map(Into::into).collect(),
            args: args.into(),
            models: Vec::new(),
        }
    }

    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }

    pub fn with_model(mut self, model: ModelSource) -> Self {
        self.models.push(model);
        self
    }

    fn check(&self) -> Vec<SpiceProblem> {
        let mut problems = Vec::new();
        let mut invalid = |reason: String| {
            problems.push(SpiceProblem::InvalidElement {
                suffix: self.suffix.clone(),
                reason,
            })
        };
        if !self.device.is_ascii_uppercase() {
            invalid(format!(
                "device letter {:?} is not an ASCII uppercase letter",
                self.device
            ));
        }
        if let Err(reason) = cuprite_wire_kicad_lib::naming::check_element_suffix(&self.suffix) {
            invalid(reason);
        }
        if let Some(reason) = check_args(&self.args) {
            invalid(format!("arguments {:?} {reason}", self.args));
        }
        for model in &self.models {
            match model {
                ModelSource::Include { path, .. } => {
                    if !path.is_absolute() {
                        invalid(format!("include path {} is not absolute", path.display()));
                    } else if path.to_str().is_none_or(|s| s.contains(['"', '\n', '\r'])) {
                        invalid(format!(
                            "include path {} cannot be written in a deck",
                            path.display()
                        ));
                    }
                }
                ModelSource::Inline { name, text, .. } => {
                    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control())
                    {
                        invalid(format!(
                            "model name {name:?} is empty or contains whitespace"
                        ));
                    }
                    if text.trim().is_empty() {
                        invalid(format!("definition of model {name:?} is empty"));
                    }
                }
            }
        }
        problems
    }
}

/// Characters that end the element line early or break it.
fn check_args(args: &str) -> Option<&'static str> {
    if args.chars().any(|c| c.is_control()) {
        Some("contain a line break or a control character")
    } else if args.contains([';', '$']) {
        Some("contain a comment character (';' or '$')")
    } else {
        None
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
    cuprite_wire_kicad_lib::value::to_spice(value, device).ok_or_else(|| {
        format!("the value {value:?} is not a number (like 10k, 4k7 or 1M for mega)")
    })
}

/// A model definition required by an element.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelSource {
    /// A model library written as `.include "<path>"`. The path must be absolute
    /// (e.g. `concat!(env!("CARGO_MANIFEST_DIR"), "/models/x.lib")`), since ngspice resolves
    /// relative paths against its working directory.
    Include {
        path: PathBuf,
        dialect: SpiceDialect,
    },
    /// A `.model` or `.subckt` definition written into the deck. Definitions are deduplicated
    /// by name (case-insensitively, like SPICE); two different definitions with the same name are an error.
    Inline {
        name: String,
        text: String,
        dialect: SpiceDialect,
    },
}

impl ModelSource {
    /// A model library in ngspice's dialect; see [`ModelSource::pspice`] for others.
    pub fn include(path: impl Into<PathBuf>) -> Self {
        ModelSource::Include {
            path: path.into(),
            dialect: SpiceDialect::Ngspice,
        }
    }

    /// A definition in ngspice's dialect; see [`ModelSource::pspice`] for others.
    pub fn inline(name: impl Into<String>, text: impl Into<String>) -> Self {
        ModelSource::Inline {
            name: name.into(),
            text: text.into(),
            dialect: SpiceDialect::Ngspice,
        }
    }

    pub fn dialect(&self) -> SpiceDialect {
        match self {
            ModelSource::Include { dialect, .. } | ModelSource::Inline { dialect, .. } => *dialect,
        }
    }

    pub fn with_dialect(mut self, new: SpiceDialect) -> Self {
        match &mut self {
            ModelSource::Include { dialect, .. } | ModelSource::Inline { dialect, .. } => {
                *dialect = new
            }
        }
        self
    }

    /// Marks the model as written for PSpice (e.g. vendor models using `IF(...)`):
    /// decks using it run ngspice in its PSpice compatibility mode.
    pub fn pspice(self) -> Self {
        self.with_dialect(SpiceDialect::Pspice)
    }

    /// Marks the model as written for LTspice: decks using it run ngspice in its LTspice
    /// compatibility mode.
    pub fn ltspice(self) -> Self {
        self.with_dialect(SpiceDialect::Ltspice)
    }
}

pub use cuprite_wire_kicad_lib::spice_lib::Dialect as SpiceDialect;

/// A problem in the SPICE information of a part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpiceProblem {
    /// `Elements` without any element (use `Excluded` to leave a part out).
    NoElements,
    /// An element cannot be written (`suffix` identifies the element).
    InvalidElement { suffix: String, reason: String },
    /// Two elements of the part have the same suffix.
    DuplicateSuffix { suffix: String },
    /// An element or `unused_pins` refers to a pin the part does not have.
    UnknownPin { pin_id: String },
    /// A pin of the part is used by no element and not declared unused.
    UnmappedPin { pin_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_parts::strings;

    #[test]
    fn test_check_pins() {
        let info = SpicePartInfo::element(SpiceElement::new('X', ["in", "gnd_a", "x"], "AMP"))
            .with_unused_pins(["nc"]);
        let pin_ids = strings(&["in", "gnd_a", "gnd_b", "nc", "out"]);
        let internal = vec![strings(&["gnd_a", "gnd_b"])];
        assert_eq!(
            info.check(&pin_ids, &internal),
            vec![
                SpiceProblem::UnknownPin { pin_id: "x".into() },
                SpiceProblem::UnmappedPin {
                    pin_id: "out".into()
                },
            ]
        );
        assert!(SpicePartInfo::Excluded.check(&pin_ids, &[]).is_empty());
    }

    #[test]
    fn test_check_elements() {
        let pin_ids = strings(&["1", "2"]);
        let info = SpicePartInfo::elements([
            SpiceElement::new('r', ["1", "2"], "10k;1%"),
            SpiceElement::new('C', ["1", "2"], "1n")
                .with_suffix("")
                .with_model(ModelSource::include("models/c.lib"))
                .with_model(ModelSource::inline("my model", "")),
        ]);
        let reasons: Vec<_> = info
            .check(&pin_ids, &[])
            .into_iter()
            .map(|problem| match problem {
                SpiceProblem::InvalidElement { reason, .. } => reason,
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            reasons,
            vec![
                "device letter 'r' is not an ASCII uppercase letter".to_string(),
                "arguments \"10k;1%\" contain a comment character (';' or '$')".into(),
                "include path models/c.lib is not absolute".into(),
                "model name \"my model\" is empty or contains whitespace".into(),
                "definition of model \"my model\" is empty".into(),
                "DuplicateSuffix { suffix: \"\" }".into(),
            ]
        );
        assert_eq!(
            SpicePartInfo::elements([]).check(&[], &[]),
            vec![SpiceProblem::NoElements]
        );
    }

    #[test]
    fn test_dialects() {
        let model = ModelSource::include("/m.lib").pspice();
        assert_eq!(model.dialect(), SpiceDialect::Pspice);
        assert_eq!(
            ModelSource::inline("m", ".model m D").dialect(),
            SpiceDialect::Ngspice
        );
    }
}
