//! KiCad S-expression netlist (`.net`, `version "E"`) export.
//!
//! See [KiCad Netlist Format](https://github.com/hyranno/cuprite_wire/blob/main/doc/dev/kicad_netlist_format.md) for the format.

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
    ///
    /// # Errors
    /// The problems found in the KiCad information (a part without KiCad information or without
    /// a footprint, a pin without KiCad pin information, pins sharing a pad, ...). All errors are collected, each with
    /// the trace of the component.
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
                // A part with two pins on one pad is rejected (`DuplicatePad`), and references
                // are unique (`DuplicateRef`).
                debug_assert!(
                    nodes
                        .windows(2)
                        .all(|pair| (&pair[0].ref_name, &pair[0].pad)
                            != (&pair[1].ref_name, &pair[1].pad)),
                    "a pad is listed twice in net {:?}",
                    net.name
                );
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
    let check = |info: &KicadPartInfo| {
        let footprint = info
            .footprint
            .is_none()
            .then(|| CircuitErrorKind::FootprintMissing {
                ref_name: ref_name(),
            });
        let pins = info
            .check_pins(&component.part.pin_ids())
            .into_iter()
            .map(|problem| match problem {
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
        footprint.into_iter().chain(pins).collect()
    };
    component.checked_info(
        component.part.kicad(),
        |ref_name| CircuitErrorKind::KicadUnsupported { ref_name },
        check,
        errors,
    )
}

impl KicadNetlist {
    /// Sets the `source` written in the `design` section (e.g. the name of the circuit description).
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub(crate) fn to_sexpr(&self) -> SExpr {
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
                SExpr::node(
                    "components",
                    self.components.iter().map(KicadComponent::to_sexpr),
                ),
                SExpr::node("nets", self.nets.iter().map(KicadNet::to_sexpr)),
            ],
        )
    }

    /// Writes the netlist to `writer` as a `.net` file, followed by a newline.
    ///
    /// # Errors
    /// The error of `writer`.
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
        let value = if self.value.is_empty() {
            "~"
        } else {
            &self.value
        };
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
                [
                    SExpr::kv("lib", lib_source.library()),
                    SExpr::kv("part", lib_source.name()),
                ],
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
            [
                SExpr::kv("names", &self.sheet_names),
                SExpr::kv("tstamps", &self.sheet_tstamps),
            ],
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
            let mut node_items = vec![
                SExpr::kv("ref", &node.ref_name),
                SExpr::kv("pin", &node.pad),
            ];
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
    use std::collections::BTreeMap;

    use petgraph::graph::NodeIndex;

    use super::*;
    use crate::{
        circuit::CircuitBuilder,
        kicad::{KicadPinInfo, PinType},
        naming::NetLabel,
        spice::IndependentSource,
        subcircuit::SubcircuitGenerator,
        test_parts::{
            self, Capacitor, CustomPart, MultiGndIc, OpAmp, RcStage, Resistor, rc_filter,
        },
    };

    fn check_golden(name: &str, netlist: &KicadNetlist) {
        let mut actual = Vec::new();
        netlist.write(&mut actual).unwrap();
        test_parts::check_golden(name, &String::from_utf8(actual).unwrap());
    }

    fn export(circuit: &CircuitBuilder) -> KicadNetlist {
        circuit.build(&()).to_netlist().unwrap().to_kicad().unwrap()
    }

    #[test]
    fn test_golden_rc_filter() {
        let netlist = export(&rc_filter()).with_source("rc_filter");
        check_golden("rc_filter.net", &netlist);
    }

    /// pcbnew detects a KiCad netlist by a line matching `[ ]*\(export[ ]+`
    /// and reports "Cannot open netlist file" otherwise.
    #[test]
    fn test_first_line_is_recognized_by_pcbnew() {
        let text = export(&rc_filter()).to_string();
        assert!(
            text.lines()
                .next()
                .unwrap()
                .starts_with("(export (version \"E\")")
        );
    }

    #[test]
    fn test_golden_hierarchy() {
        let mut circuit = CircuitBuilder::new();
        let first = circuit.instantiate_subcircuit_named("stage1", RcStage);
        let second = circuit.instantiate_subcircuit(RcStage);
        circuit.connect(first[1], second[0]);
        circuit.label(first[0], NetLabel::local("IN"));
        let test_point = circuit.instantiate_part_named(
            "test_point",
            Box::new(CustomPart {
                pin_ids: vec!["1"],
                kicad: Some(KicadPartInfo {
                    footprint: Some("TestPoint:TestPoint_Pad_D1.0mm".parse().unwrap()),
                    pins: BTreeMap::from([(
                        "1".into(),
                        KicadPinInfo::new("1", Some("1"), PinType::Passive),
                    )]),
                    fields: vec![
                        ("Note".into(), "probe \"here\"".into()),
                        ("Empty".into(), String::new()),
                    ],
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
        circuit.connect(test_point["1"], second[1]);

        let netlist = export(&circuit);
        check_golden("hierarchy.net", &netlist);
        // The output is deterministic.
        assert_eq!(netlist.to_string(), export(&circuit).to_string());
    }

    /// Node 0: input, node 1: output (labeled "X" inside).
    #[derive(Debug, Clone)]
    struct Inner;
    impl SubcircuitGenerator for Inner {
        type Options = ();
        type Interface = [NodeIndex; 2];

        fn construct_subcircuit(
            &self,
            _options: &(),
            circuit: &mut CircuitBuilder<()>,
            interface: Self::Interface,
        ) {
            let [input, output] = interface;
            let r_a = circuit.instantiate_part_named("a", Box::new(Resistor("1k")));
            let r_b = circuit.instantiate_part(Box::new(Resistor("2k")));
            let c = circuit.instantiate_part(Box::new(Capacitor("1n")));
            circuit.connect(input, r_a["1"]);
            circuit.connect(r_a["2"], r_b["1"]);
            circuit.connect(r_a["2"], c["1"]);
            circuit.label(c["2"], NetLabel::global("VCC"));
            circuit.connect(r_b["2"], output);
            circuit.label(output, NetLabel::local("X"));
        }
    }

    /// Node 0: input, node 1: output. Contains an `Inner`.
    #[derive(Debug, Clone)]
    struct Outer;
    impl SubcircuitGenerator for Outer {
        type Options = ();
        type Interface = [NodeIndex; 2];

        fn construct_subcircuit(
            &self,
            _options: &(),
            circuit: &mut CircuitBuilder<()>,
            interface: Self::Interface,
        ) {
            let [input, output] = interface;
            let inner = circuit.instantiate_subcircuit_named("inner", Inner);
            let r_o = circuit.instantiate_part_named("o", Box::new(Resistor("3k")));
            circuit.connect(input, inner[0]);
            circuit.connect(inner[1], r_o["1"]);
            circuit.connect(r_o["2"], output);
        }
    }

    /// Nested subcircuits: nets inside a subcircuit, labels reaching up, and a net between
    /// subcircuits without a name.
    #[test]
    fn test_golden_nested_hierarchy() {
        let mut circuit = CircuitBuilder::new();
        let first = circuit.instantiate_subcircuit_named("outer", Outer);
        let second = circuit.instantiate_subcircuit(Outer);
        let r_root = circuit.instantiate_part_named("root", Box::new(Resistor("10k")));
        circuit.label(r_root["1"], NetLabel::local("IN"));
        circuit.connect(r_root["1"], first[0]);
        circuit.connect(first[1], second[0]);
        circuit.label(r_root["2"], NetLabel::global("VCC"));
        check_golden("nested_hierarchy.net", &export(&circuit));
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

    /// Net names in mixed case, digits and symbols, to compare the order with KiCad's
    /// (`tools/compare_kicad_cli_netlist.py`).
    #[test]
    fn test_golden_net_order() {
        let mut circuit = CircuitBuilder::new();
        let labels = [
            NetLabel::local("ZZZ"),
            NetLabel::local("first"),
            NetLabel::local("a10"),
            NetLabel::local("a9"),
            NetLabel::local("b"),
            NetLabel::local("B"),
            NetLabel::local("_x"),
            NetLabel::local("Z9"),
            NetLabel::local("z10"),
            NetLabel::global("vcc"),
            NetLabel::global("GND"),
            NetLabel::global("+3V3"),
        ];
        for label in labels {
            let r = circuit.instantiate_part(Box::new(Resistor("1k")));
            circuit.label(r["1"], label);
        }
        check_golden("net_order.net", &export(&circuit));
    }

    /// Versions of one circuit, for checking how pcbnew links footprints when the circuit changes
    /// (`tools/check_netlist_import.py --relink`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Revision {
        Before,
        /// A part with an explicit name is added before the others: references are renumbered.
        NamedAdded,
        /// A part with an automatic name is added before another one: the automatic names shift.
        AutoInserted,
    }

    fn revised_circuit(revision: Revision) -> CircuitBuilder {
        let mut circuit = CircuitBuilder::new();
        let input = circuit.add_node();
        circuit.label(input, NetLabel::local("IN"));
        if revision == Revision::NamedAdded {
            let r_in = circuit.instantiate_part_named("r_in", Box::new(Resistor("1k")));
            circuit.connect(input, r_in["1"]);
            let input = circuit.add_node();
            circuit.connect(r_in["2"], input);
        }
        let r = circuit.instantiate_part_named("r", Box::new(Resistor("10k")));
        let gnd = circuit.add_node();
        circuit.label(gnd, NetLabel::global("GND"));
        if revision == Revision::AutoInserted {
            let r_new = circuit.instantiate_part(Box::new(Resistor("47k")));
            circuit.connect(r_new["1"], r["2"]);
            circuit.connect(r_new["2"], gnd);
        }
        let r_load = circuit.instantiate_part(Box::new(Resistor("22k")));
        let c = circuit.instantiate_part_named("c", Box::new(Capacitor("100n")));
        circuit.connect(input, r["1"]);
        circuit.connect(r["2"], c["1"]);
        circuit.connect(c["1"], r_load["1"]);
        circuit.label(c["1"], NetLabel::local("OUT"));
        circuit.connect(c["2"], gnd);
        circuit.connect(r_load["2"], gnd);
        circuit
    }

    #[test]
    fn test_golden_revisions() {
        let before = export(&revised_circuit(Revision::Before));
        let named_added = export(&revised_circuit(Revision::NamedAdded));
        let auto_inserted = export(&revised_circuit(Revision::AutoInserted));
        check_golden("revision_before.net", &before);
        check_golden("revision_named_added.net", &named_added);
        check_golden("revision_auto_inserted.net", &auto_inserted);

        let by_uuid = |netlist: &KicadNetlist| -> BTreeMap<Uuid, (String, String)> {
            netlist
                .components
                .iter()
                .map(|c| (c.uuid, (c.ref_name.clone(), c.value.clone())))
                .collect()
        };
        let before = by_uuid(&before);
        let named_added = by_uuid(&named_added);
        let auto_inserted = by_uuid(&auto_inserted);
        let pairs = |map: &BTreeMap<Uuid, (String, String)>| -> Vec<(String, String)> {
            before
                .keys()
                .map(|uuid| map.get(uuid).cloned().unwrap_or_default())
                .collect()
        };
        // Before: R1 = r (10k), R2 = R#1 (22k), C1 = c.
        let v = |r: &str, value: &str| (r.to_string(), value.to_string());
        let mut expected = vec![v("C1", "100n"), v("R1", "10k"), v("R2", "22k")];
        let mut actual: Vec<_> = before.values().cloned().collect();
        actual.sort();
        assert_eq!(actual, expected);
        // Every part keeps its UUID, though R1 and R2 are renumbered to R2 and R3.
        let mut actual = pairs(&named_added);
        actual.sort();
        expected = vec![v("C1", "100n"), v("R2", "10k"), v("R3", "22k")];
        assert_eq!(actual, expected);
        // The new part takes the automatic name R#1, and with it the UUID of the 22k resistor.
        let mut actual = pairs(&auto_inserted);
        actual.sort();
        expected = vec![v("C1", "100n"), v("R1", "10k"), v("R2", "47k")];
        assert_eq!(actual, expected);
    }

    /// Instantiates a part and records the line of the caller.
    #[track_caller]
    fn add(
        circuit: &mut CircuitBuilder,
        part: impl crate::parts::Part + 'static,
        lines: &mut Vec<u32>,
    ) {
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

        add(&mut circuit, *IndependentSource::voltage(5.0), &mut lines);
        add(
            &mut circuit,
            CustomPart {
                kicad: Some(no_footprint),
                ..valid()
            },
            &mut lines,
        );
        add(
            &mut circuit,
            CustomPart {
                pin_ids: vec!["1", "2", "3"],
                ..valid()
            },
            &mut lines,
        );
        add(
            &mut circuit,
            CustomPart {
                pin_ids: vec!["1"],
                ..valid()
            },
            &mut lines,
        );
        add(
            &mut circuit,
            CustomPart {
                kicad: Some(duplicate_pad),
                ..valid()
            },
            &mut lines,
        );

        // Parts without KiCad information do not prevent creating the netlist.
        let netlist = circuit.build(&()).to_netlist().unwrap();
        let errors = netlist.to_kicad().unwrap_err().0;
        let kinds: Vec<_> = errors.iter().map(|e| e.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                CircuitErrorKind::KicadUnsupported {
                    ref_name: "V1".into()
                },
                CircuitErrorKind::FootprintMissing {
                    ref_name: "X1".into()
                },
                CircuitErrorKind::PinInfoMissing {
                    ref_name: "X2".into(),
                    pin_id: "3".into()
                },
                CircuitErrorKind::UnknownPinId {
                    ref_name: "X3".into(),
                    pin_id: "2".into()
                },
                CircuitErrorKind::DuplicatePad {
                    ref_name: "X4".into(),
                    pad: "1".into()
                },
            ]
        );
        let error_lines: Vec<_> = errors
            .iter()
            .map(|e| e.trace.innermost().unwrap().line())
            .collect();
        // Components are sorted by reference: V1, X1, X2, X3, X4.
        assert_eq!(error_lines, lines);
        let message = CircuitErrors(errors).to_string();
        assert!(message.contains(&format!(
            "error: V1: the part cannot be exported to KiCad\n  at {}:{}:",
            file!(),
            lines[0]
        )));
    }
}
