//! KiCad S-expression netlist (`.net`, `version "E"`) export.
//!
//! See `doc/kicad_netlist_format.md` for the format.

use std::{fmt, io};

use uuid::Uuid;

use crate::{
    error::{CircuitError, CircuitErrorKind, CircuitErrors},
    kicad::{KicadPartInfo, LibId, PartFlags, PinInfoProblem},
    netlist::{Component, Netlist},
    sexpr::SExpr,
};

/// A netlist checked and resolved for KiCad. Write it with [`KicadNetlist::write`] or `Display`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KicadNetlist {
    source: Option<String>,
    components: Vec<KicadComponent>,
    nets: Vec<KicadNet>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KicadComponent {
    ref_name: String,
    value: String,
    footprint: LibId,
    fields: Vec<(String, String)>,
    lib_source: Option<LibId>,
    flags: PartFlags,
    sheet_names: String,
    sheet_tstamps: String,
    uuid: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KicadNet {
    code: usize,
    name: String,
    nodes: Vec<KicadNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct KicadNode {
    ref_name: String,
    pad: String,
    pin_function: Option<String>,
    pin_type: &'static str,
}

impl Netlist {
    /// Resolves the KiCad information of every component and checks it.
    /// All errors are collected, each with the trace of the component.
    pub fn to_kicad(&self) -> Result<KicadNetlist, CircuitErrors> {
        let mut errors = Vec::new();
        let infos: Vec<Option<KicadPartInfo>> = self
            .components
            .iter()
            .map(|component| check_component(component, &mut errors))
            .collect();

        let components = self
            .components
            .iter()
            .zip(&infos)
            .filter_map(|(component, info)| {
                let info = info.as_ref()?;
                Some(KicadComponent {
                    ref_name: component.ref_name.clone(),
                    value: component.part.value(),
                    footprint: info.footprint.clone()?,
                    fields: info.fields.clone(),
                    lib_source: info.lib_source.clone(),
                    flags: info.flags,
                    sheet_names: component.path.to_string(),
                    sheet_tstamps: component.path.tstamps(),
                    uuid: component.uuid,
                })
            })
            .collect();

        let nets = self
            .nets
            .iter()
            .enumerate()
            .map(|(i, net)| {
                let mut nodes: Vec<KicadNode> = net
                    .nodes
                    .iter()
                    .filter_map(|node| {
                        let pin = infos[node.component].as_ref()?.pins.get(&node.pin_id)?;
                        Some(KicadNode {
                            ref_name: self.components[node.component].ref_name.clone(),
                            pad: pin.pad.clone(),
                            pin_function: pin.name.clone().filter(|name| !name.is_empty()),
                            pin_type: pin.pin_type.as_str(),
                        })
                    })
                    .collect();
                // KiCad sorts nodes by reference and pad number, both in string order.
                nodes.sort();
                nodes.dedup_by(|a, b| a.ref_name == b.ref_name && a.pad == b.pad);
                KicadNet {
                    code: i + 1,
                    name: net.name.clone(),
                    nodes,
                }
            })
            .collect();

        CircuitErrors::into_result(
            errors,
            KicadNetlist {
                source: None,
                components,
                nets,
            },
        )
    }
}

/// Returns the KiCad information of a component if it can be exported.
fn check_component(component: &Component, errors: &mut Vec<CircuitError>) -> Option<KicadPartInfo> {
    let ref_name = || component.ref_name.clone();
    let errors_before = errors.len();
    let mut error = |kind| errors.push(CircuitError::new(kind, component.trace.clone()));

    let Some(info) = component.part.kicad() else {
        error(CircuitErrorKind::KicadUnsupported { ref_name: ref_name() });
        return None;
    };
    if info.footprint.is_none() {
        error(CircuitErrorKind::FootprintMissing { ref_name: ref_name() });
    }
    for problem in info.check_pins(&component.part.pin_ids()) {
        error(match problem {
            PinInfoProblem::Missing { pin_id } => CircuitErrorKind::PinInfoMissing {
                ref_name: ref_name(),
                pin_id,
            },
            PinInfoProblem::Unknown { pin_id } => CircuitErrorKind::UnknownPinId {
                ref_name: ref_name(),
                pin_id,
            },
            PinInfoProblem::DuplicatePad { pad } => CircuitErrorKind::DuplicatePad {
                ref_name: ref_name(),
                pad,
            },
        });
    }
    (errors.len() == errors_before).then_some(info)
}

impl KicadNetlist {
    /// Sets the `source` written in the `design` section (e.g. the name of the circuit description).
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn to_sexpr(&self) -> SExpr {
        let mut design = Vec::new();
        if let Some(source) = &self.source {
            design.push(SExpr::kv("source", source));
        }
        design.push(SExpr::kv(
            "tool",
            format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        ));
        SExpr::node(
            "export",
            [
                SExpr::kv("version", "E"),
                SExpr::node("design", design),
                SExpr::node("components", self.components.iter().map(KicadComponent::to_sexpr)),
                SExpr::node("nets", self.nets.iter().map(KicadNet::to_sexpr)),
            ],
        )
    }

    pub fn write(&self, writer: &mut impl io::Write) -> io::Result<()> {
        writeln!(writer, "{self}")
    }
}

impl fmt::Display for KicadNetlist {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_sexpr())
    }
}

impl KicadComponent {
    fn to_sexpr(&self) -> SExpr {
        let value = if self.value.is_empty() { "~" } else { &self.value };
        let mut items = vec![
            SExpr::kv("ref", &self.ref_name),
            SExpr::kv("value", value),
            SExpr::kv("footprint", self.footprint.to_string()),
        ];
        if !self.fields.is_empty() {
            items.push(SExpr::node(
                "fields",
                self.fields.iter().map(|(name, value)| {
                    let mut field = vec![SExpr::kv("name", name)];
                    if !value.is_empty() {
                        field.push(SExpr::string(value));
                    }
                    SExpr::node("field", field)
                }),
            ));
        }
        if let Some(lib_source) = &self.lib_source {
            items.push(SExpr::node(
                "libsource",
                [SExpr::kv("lib", lib_source.library()), SExpr::kv("part", lib_source.name())],
            ));
        }
        let flags = [
            (self.flags.exclude_from_board, "exclude_from_board"),
            (self.flags.exclude_from_bom, "exclude_from_bom"),
            (self.flags.dnp, "dnp"),
        ];
        for (_, name) in flags.iter().filter(|(set, _)| *set) {
            items.push(SExpr::node("property", [SExpr::kv("name", *name)]));
        }
        items.push(SExpr::node(
            "sheetpath",
            [SExpr::kv("names", &self.sheet_names), SExpr::kv("tstamps", &self.sheet_tstamps)],
        ));
        items.push(SExpr::kv("tstamps", self.uuid.to_string()));
        SExpr::node("comp", items)
    }
}

impl KicadNet {
    fn to_sexpr(&self) -> SExpr {
        let mut items = vec![
            SExpr::kv("code", self.code.to_string()),
            SExpr::kv("name", &self.name),
            SExpr::kv("class", "Default"),
        ];
        items.extend(self.nodes.iter().map(|node| {
            let mut node_items = vec![SExpr::kv("ref", &node.ref_name), SExpr::kv("pin", &node.pad)];
            if let Some(pin_function) = &node.pin_function {
                node_items.push(SExpr::kv("pinfunction", pin_function));
            }
            node_items.push(SExpr::kv("pintype", node.pin_type));
            SExpr::node("node", node_items)
        }));
        SExpr::node("net", items)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashSet},
        path::Path,
    };

    use petgraph::graph::NodeIndex;

    use super::*;
    use crate::{
        circuit::CircuitBuilder,
        kicad::{KicadPinInfo, PinType},
        naming::NetLabel,
        subcircuit::SubcircuitGenerator,
        test_parts::{Capacitor, CustomPart, MultiGndIc, OpAmp, Resistor, SpiceSource},
    };

    /// Compares the output with `tests/golden/<name>`.
    /// Run the tests with `UPDATE_GOLDEN=1` to (re)generate the files.
    fn check_golden(name: &str, netlist: &KicadNetlist) {
        let mut actual = Vec::new();
        netlist.write(&mut actual).unwrap();
        let actual = String::from_utf8(actual).unwrap();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("{} is missing; run with UPDATE_GOLDEN=1", path.display()));
        assert_eq!(actual, expected, "output differs from {}", path.display());
    }

    fn export(circuit: &CircuitBuilder) -> KicadNetlist {
        circuit.build(&()).to_netlist().unwrap().to_kicad().unwrap()
    }

    fn rc_filter() -> CircuitBuilder {
        let mut circuit = CircuitBuilder::new();
        let r = circuit.instantiate_part_named("r", Box::new(Resistor("10k")));
        let c = circuit.instantiate_part_named("c", Box::new(Capacitor("100n")));
        circuit.label(r["1"], NetLabel::local("IN"));
        circuit.connect(r["2"], c["1"]);
        circuit.label(c["1"], NetLabel::local("OUT"));
        circuit.label(c["2"], NetLabel::global("GND"));
        circuit
    }

    #[test]
    fn test_golden_rc_filter() {
        let netlist = export(&rc_filter()).with_source("rc_filter");
        check_golden("rc_filter.net", &netlist);
    }

    /// An RC low-pass stage. Node 0: input, node 1: output.
    #[derive(Debug, Clone)]
    struct RcStage;
    impl SubcircuitGenerator for RcStage {
        type Options = ();

        fn construct_subcircuit(&self, _options: &()) -> CircuitBuilder<()> {
            let mut circuit = CircuitBuilder::new();
            let input = circuit.add_node();
            let output = circuit.add_node();
            let r = circuit.instantiate_part(Box::new(Resistor("1k")));
            let c = circuit.instantiate_part(Box::new(Capacitor("1u")));
            circuit.connect(input, r["1"]);
            circuit.connect(r["2"], output);
            circuit.connect(output, c["1"]);
            circuit.label(output, NetLabel::local("OUT"));
            circuit.label(c["2"], NetLabel::global("GND"));
            circuit
        }
    }

    #[test]
    fn test_golden_hierarchy() {
        let mut circuit = CircuitBuilder::new();
        let interface = HashSet::from([NodeIndex::new(0), NodeIndex::new(1)]);
        let first = circuit.instantiate_subcircuit_named("stage1", RcStage, &interface, |o: &()| *o);
        let second = circuit.instantiate_subcircuit(RcStage, &interface, |o: &()| *o);
        circuit.connect(first[&NodeIndex::new(1)], second[&NodeIndex::new(0)]);
        circuit.label(first[&NodeIndex::new(0)], NetLabel::local("IN"));
        let test_point = circuit.instantiate_part_named(
            "test_point",
            Box::new(CustomPart {
                pin_ids: vec!["1"],
                kicad: Some(KicadPartInfo {
                    footprint: Some("TestPoint:TestPoint_Pad_D1.0mm".parse().unwrap()),
                    pins: BTreeMap::from([("1".into(), KicadPinInfo::new("1", Some("1"), PinType::Passive))]),
                    fields: vec![("Note".into(), "probe \"here\"".into()), ("Empty".into(), String::new())],
                    flags: PartFlags {
                        dnp: true,
                        exclude_from_bom: true,
                        exclude_from_board: false,
                    },
                    ..Default::default()
                }),
                ..CustomPart::valid()
            }),
        );
        circuit.connect(test_point["1"], second[&NodeIndex::new(1)]);

        let netlist = export(&circuit);
        check_golden("hierarchy.net", &netlist);
        // The output is deterministic.
        assert_eq!(netlist.to_string(), export(&circuit).to_string());
    }

    #[test]
    fn test_golden_multi_pin_ic() {
        let mut circuit = CircuitBuilder::new();
        let u_amp = circuit.instantiate_part_named("amp", Box::new(OpAmp));
        let u_ic = circuit.instantiate_part_named("ic", Box::new(MultiGndIc));
        let r_feedback = circuit.instantiate_part_named("feedback", Box::new(Resistor("100k")));
        circuit.connect(u_amp["out"], r_feedback["1"]);
        circuit.connect(r_feedback["2"], u_amp["in_n"]);
        circuit.connect(u_amp["out"], u_ic["io"]);
        circuit.label(u_amp["vss"], NetLabel::global("GND"));
        circuit.label(u_ic["gnd_b"], NetLabel::global("GND"));
        circuit.label(u_amp["vdd"], NetLabel::global("+3V3"));
        circuit.label(u_ic["vcc"], NetLabel::global("+3V3"));

        check_golden("multi_pin_ic.net", &export(&circuit));
    }

    /// Instantiates a part and records the line of the caller.
    #[track_caller]
    fn add(circuit: &mut CircuitBuilder, part: impl crate::parts::Part + 'static, lines: &mut Vec<u32>) {
        lines.push(std::panic::Location::caller().line());
        circuit.instantiate_part(Box::new(part));
    }

    #[test]
    fn test_kicad_errors_are_collected() {
        let mut circuit = CircuitBuilder::new();
        let mut lines = Vec::new();
        let valid = CustomPart::valid;
        let no_footprint = KicadPartInfo {
            footprint: None,
            ..valid().kicad.unwrap()
        };
        let mut duplicate_pad = valid().kicad.unwrap();
        duplicate_pad.pins.get_mut("2").unwrap().pad = "1".into();

        add(&mut circuit, SpiceSource, &mut lines);
        add(&mut circuit, CustomPart { kicad: Some(no_footprint), ..valid() }, &mut lines);
        add(&mut circuit, CustomPart { pin_ids: vec!["1", "2", "3"], ..valid() }, &mut lines);
        add(&mut circuit, CustomPart { pin_ids: vec!["1"], ..valid() }, &mut lines);
        add(&mut circuit, CustomPart { kicad: Some(duplicate_pad), ..valid() }, &mut lines);

        // Parts without KiCad information do not prevent creating the netlist.
        let netlist = circuit.build(&()).to_netlist().unwrap();
        let errors = netlist.to_kicad().unwrap_err().0;
        let kinds: Vec<_> = errors.iter().map(|e| e.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                CircuitErrorKind::KicadUnsupported { ref_name: "V1".into() },
                CircuitErrorKind::FootprintMissing { ref_name: "X1".into() },
                CircuitErrorKind::PinInfoMissing { ref_name: "X2".into(), pin_id: "3".into() },
                CircuitErrorKind::UnknownPinId { ref_name: "X3".into(), pin_id: "2".into() },
                CircuitErrorKind::DuplicatePad { ref_name: "X4".into(), pad: "1".into() },
            ]
        );
        let error_lines: Vec<_> = errors.iter().map(|e| e.trace.innermost().unwrap().line()).collect();
        // Components are sorted by reference: V1, X1, X2, X3, X4.
        assert_eq!(error_lines, lines);
        let message = CircuitErrors(errors).to_string();
        assert!(message.contains(&format!("error: V1: the part cannot be exported to KiCad\n  at {}:{}:", file!(), lines[0])));
    }
}
