//! Format-independent netlist, the common input of the KiCad and SPICE exporters.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use petgraph::graph::NodeIndex;
use uuid::Uuid;

use crate::{
    circuit::BuiltCircuit,
    erc::NodeMark,
    error::{
        CircuitError, CircuitErrorKind, CircuitErrors, CircuitWarning, CircuitWarningKind, warn,
    },
    kicad::KicadPartInfo,
    naming::{HierPath, NetLabel, NodeLabel, escape_name, natural_cmp},
    parts::Part,
    trace::Trace,
};

/// A resolved netlist. Components are grouped by subcircuit instance (in the order of [`sheet_order`])
/// and sorted by reference in each, and nets are sorted by name (both in natural order).
#[derive(Debug, Clone)]
pub struct Netlist {
    pub components: Vec<Component>,
    pub nets: Vec<Net>,
    /// ERC marks on nets without part pins (such nets are not in [`Netlist::nets`]).
    pub pinless_marks: Vec<NodeMark>,
    pub warnings: Vec<CircuitWarning>,
}

#[derive(Debug, Clone)]
pub struct Component {
    pub ref_name: String,
    pub part: Box<dyn Part>,
    /// Path of the subcircuit instance containing the component.
    pub path: HierPath,
    pub uuid: Uuid,
    /// Where the component was instantiated.
    pub trace: Trace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Net {
    pub name: String,
    /// Sorted by component, then by pin ID.
    pub nodes: Vec<NetNode>,
    /// The location of the label giving the name, or of the first node for unnamed nets.
    pub trace: Trace,
    /// ERC marks attached to the net. The exporters ignore them.
    pub marks: Vec<NodeMark>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetNode {
    /// Index into [`Netlist::components`].
    pub component: usize,
    pub pin_id: String,
}

impl BuiltCircuit {
    /// Resolves nets and their names, and checks the circuit independently of the output format.
    ///
    /// All errors are collected, including those found while describing the circuit.
    /// Warnings are reported and also kept in [`Netlist::warnings`].
    pub fn to_netlist(&self) -> Result<Netlist, CircuitErrors> {
        let mut errors = self.errors.clone();
        let mut warnings = Vec::new();

        let (components, component_of_part) = self.sorted_components(&mut errors);
        // KiCad pin names and pads are used for naming unnamed nets, unless they are inconsistent
        // (the KiCad exporter reports that), so that the names stay unique.
        let kicad_infos: Vec<_> = components
            .iter()
            .map(|c| {
                c.part
                    .kicad()
                    .filter(|info| info.check_pins(&c.part.pin_ids()).is_empty())
            })
            .collect();
        let pins_by_node = self.pins_by_node(&component_of_part);

        let mut nets = Vec::new();
        let mut pinless_marks = Vec::new();
        for group in self.connected_nodes() {
            let mut nodes: Vec<NetNode> = group
                .iter()
                .filter_map(|node| pins_by_node.get(node).cloned())
                .collect();
            let labels: Vec<&NodeLabel> = group
                .iter()
                .flat_map(|&node| &self.nets[node].labels)
                .collect();
            let marks: Vec<NodeMark> = group
                .iter()
                .flat_map(|&node| self.nets[node].marks.iter().cloned())
                .collect();
            if nodes.is_empty() {
                pinless_marks.extend(marks);
                if let Some(first) = labels.first() {
                    let names = distinct_names(&labels);
                    warnings.push(CircuitWarning::new(
                        CircuitWarningKind::LabelOnlyNet { names },
                        first.trace.clone(),
                    ));
                }
                continue;
            }
            nodes.sort();

            let net = match choose_label(&labels) {
                Some(label) => {
                    let name = label.net_name();
                    let others: Vec<_> = distinct_names(&labels)
                        .into_iter()
                        .filter(|other| *other != name)
                        .collect();
                    if !others.is_empty() {
                        let mut warning = CircuitWarning::new(
                            CircuitWarningKind::MultipleNetNames {
                                used: name.clone(),
                                others,
                            },
                            label.trace.clone(),
                        );
                        for other in labels.iter().filter(|other| other.net_name() != name) {
                            warning = warning.with_note(
                                format!("also named {:?} here", other.net_name()),
                                other.trace.clone(),
                            );
                        }
                        warnings.push(warning);
                    }
                    Net {
                        name,
                        nodes,
                        trace: label.trace.clone(),
                        marks,
                    }
                }
                None => Net {
                    name: unnamed_net_name(&nodes, &components, &kicad_infos),
                    trace: self.nets[group[0]].trace.clone(),
                    nodes,
                    marks,
                },
            };
            nets.push(net);
        }
        nets.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        for pair in nets.windows(2) {
            if pair[0].name == pair[1].name {
                errors.push(
                    CircuitError::new(
                        CircuitErrorKind::DuplicateNetName {
                            name: pair[1].name.clone(),
                        },
                        pair[1].trace.clone(),
                    )
                    .with_note("the other net is named here", pair[0].trace.clone()),
                );
            }
        }

        for warning in &warnings {
            warn(warning);
        }
        CircuitErrors::into_result(
            errors,
            Netlist {
                components,
                nets,
                pinless_marks,
                warnings,
            },
        )
    }

    /// Components in the order of the netlist, after checking the parts,
    /// and the component index of each part.
    fn sorted_components(&self, errors: &mut Vec<CircuitError>) -> (Vec<Component>, Vec<usize>) {
        let ref_names: Vec<_> = self.parts.iter().map(|part| part.ref_name()).collect();
        let sheet_rank = sheet_order(self.parts.iter().map(|part| &part.path));
        let mut order: Vec<usize> = (0..self.parts.len()).collect();
        order.sort_by(|&a, &b| {
            sheet_rank[&self.parts[a].path]
                .cmp(&sheet_rank[&self.parts[b].path])
                .then_with(|| natural_cmp(&ref_names[a], &ref_names[b]))
        });
        let mut component_of_part = vec![0; self.parts.len()];
        for (component, &part) in order.iter().enumerate() {
            component_of_part[part] = component;
        }
        let components: Vec<Component> = order
            .into_iter()
            .map(|i| {
                let part = &self.parts[i];
                Component {
                    ref_name: ref_names[i].clone(),
                    part: part.part.clone(),
                    path: part.path.clone(),
                    uuid: part.uuid(),
                    trace: part.trace.clone(),
                }
            })
            .collect();

        let mut by_ref: Vec<&Component> = components.iter().collect();
        by_ref.sort_by(|a, b| natural_cmp(&a.ref_name, &b.ref_name));
        for pair in by_ref.windows(2) {
            if pair[0].ref_name == pair[1].ref_name {
                errors.push(
                    CircuitError::new(
                        CircuitErrorKind::DuplicateRef {
                            ref_name: pair[1].ref_name.clone(),
                        },
                        pair[1].trace.clone(),
                    )
                    .with_note("the other part is instantiated here", pair[0].trace.clone()),
                );
            }
        }
        for component in &components {
            let error = |kind| CircuitError::new(kind, component.trace.clone());
            let pin_ids = component.part.pin_ids();
            let mut seen = HashSet::new();
            for pin_id in &pin_ids {
                if !seen.insert(pin_id) {
                    errors.push(error(CircuitErrorKind::DuplicatePinId {
                        ref_name: component.ref_name.clone(),
                        pin_id: pin_id.clone(),
                    }));
                }
            }
            for pin_id in component.part.internal_connections().into_iter().flatten() {
                if !seen.contains(&pin_id) {
                    errors.push(error(CircuitErrorKind::UnknownPinId {
                        ref_name: component.ref_name.clone(),
                        pin_id,
                    }));
                }
            }
        }
        (components, component_of_part)
    }

    fn pins_by_node(&self, component_of_part: &[usize]) -> HashMap<NodeIndex, NetNode> {
        self.parts
            .iter()
            .zip(component_of_part)
            .flat_map(|(part, &component)| {
                part.pin_mapping.iter().map(move |(pin_id, &node)| {
                    (
                        node,
                        NetNode {
                            component,
                            pin_id: pin_id.clone(),
                        },
                    )
                })
            })
            .collect()
    }
}

/// Ranks subcircuit instances (sheets) by their first part, given the paths of the parts in
/// instantiation order. Parts are expanded depth-first, so the root comes first and each
/// subcircuit instance is followed by the ones inside it, like the pages of a KiCad schematic.
/// Eeschema writes components sheet by sheet in page order.
fn sheet_order<'a>(paths: impl Iterator<Item = &'a HierPath>) -> HashMap<&'a HierPath, usize> {
    let mut rank = HashMap::new();
    for path in paths {
        let next = rank.len();
        rank.entry(path).or_insert(next);
    }
    rank
}

/// Picks the label naming a net: global labels first, then labels in shallower
/// subcircuit instances, then the smallest name.
fn choose_label<'a>(labels: &[&'a NodeLabel]) -> Option<&'a NodeLabel> {
    let rank = |label: &NodeLabel| match label.label {
        NetLabel::Global(_) => 0,
        NetLabel::Local(_) => 1,
    };
    labels.iter().copied().min_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.scope.0.len().cmp(&b.scope.0.len()))
            .then_with(|| a.net_name().cmp(&b.net_name()))
    })
}

fn distinct_names(labels: &[&NodeLabel]) -> Vec<String> {
    let mut names: Vec<_> = labels.iter().map(|label| label.net_name()).collect();
    names.sort();
    names.dedup();
    names
}

/// Names a net without labels after one of its pins, like KiCad:
/// `Net-(R1-Pad2)` / `Net-(U1-OUT)`, or `unconnected-(U1-NC-Pad7)` for a single pin.
/// A pin without a name, or named as its pad number, is written as the pad (`Pad2`), and a pin
/// name shared by several pins of the part is followed by the pad (`Net-(U1-GND-Pad5)`).
///
/// As KiCad does, a pin written by its name alone is preferred to one written with the pad,
/// and among those the smallest name in string order (not natural order) is chosen.
///
/// Pin names and pad numbers come from the KiCad information of the parts. Pins of parts
/// without it are used only when no pin has it, and are named by their pin IDs.
fn unnamed_net_name(
    nodes: &[NetNode],
    components: &[Component],
    kicad_infos: &[Option<KicadPartInfo>],
) -> String {
    let unconnected = nodes.len() == 1;
    let prefix = if unconnected { "unconnected" } else { "Net" };
    // (written with the pad, net name) of a pin with KiCad information.
    let candidate = |node: &NetNode| {
        let info = kicad_infos[node.component].as_ref()?;
        let pin = info.pins.get(&node.pin_id)?;
        let ref_name = escape_name(&components[node.component].ref_name);
        let pad = format!("Pad{}", escape_name(&pin.pad));
        let name = pin
            .name
            .as_deref()
            .filter(|name| !name.is_empty() && *name != pin.pad);
        let (with_pad, pin_label) = match name {
            Some(name) if unconnected || info.is_pin_name_shared(name) => {
                (true, format!("{}-{pad}", escape_name(name)))
            }
            Some(name) => (false, escape_name(name)),
            None => (true, pad),
        };
        Some((with_pad, format!("{prefix}-({ref_name}-{pin_label})")))
    };
    nodes
        .iter()
        .filter_map(candidate)
        .min()
        .map(|(_, name)| name)
        .unwrap_or_else(|| {
            // `nodes` is sorted, so the first node is the smallest.
            let node = &nodes[0];
            let ref_name = escape_name(&components[node.component].ref_name);
            format!("{prefix}-({ref_name}-{})", escape_name(&node.pin_id))
        })
}

impl Ord for NetNode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.component
            .cmp(&other.component)
            .then_with(|| natural_cmp(&self.pin_id, &other.pin_id))
    }
}

impl PartialOrd for NetNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use super::*;
    use crate::{
        circuit::CircuitBuilder,
        kicad::{KicadPinInfo, PinType},
        spice::IndependentSource,
        subcircuit::SubcircuitGenerator,
        test_parts::{Capacitor, CustomPart, MultiGndIc, OpAmp, Resistor},
    };

    fn summary(netlist: &Netlist) -> Vec<(String, Vec<String>)> {
        netlist
            .nets
            .iter()
            .map(|net| {
                let nodes = net
                    .nodes
                    .iter()
                    .map(|node| {
                        format!(
                            "{}.{}",
                            netlist.components[node.component].ref_name, node.pin_id
                        )
                    })
                    .collect();
                (net.name.clone(), nodes)
            })
            .collect()
    }

    fn expected(nets: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        nets.iter()
            .map(|(name, nodes)| {
                (
                    name.to_string(),
                    nodes.iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn test_net_names_and_order() {
        let mut circuit = CircuitBuilder::<()>::new();
        let u = circuit.instantiate_part(Box::new(OpAmp));
        let r = circuit.instantiate_part(Box::new(Resistor("10k")));
        let c = circuit.instantiate_part(Box::new(Capacitor("100n")));
        circuit.label(r["1"], NetLabel::local("IN"));
        circuit.connect(r["2"], c["1"]);
        circuit.connect(c["1"], u["in_p"]);
        circuit.label(c["2"], NetLabel::global("GND"));
        circuit.label(u["vss"], NetLabel::global("GND"));
        circuit.connect(u["out"], u["in_n"]);
        let netlist = circuit.build(&()).to_netlist().unwrap();

        let refs: Vec<_> = netlist
            .components
            .iter()
            .map(|c| c.ref_name.as_str())
            .collect();
        assert_eq!(refs, vec!["C1", "R1", "U1"]);
        assert_eq!(
            summary(&netlist),
            expected(&[
                ("/IN", &["R1.1"]),
                ("GND", &["C1.2", "U1.vss"]),
                // Named pins are preferred, then the smallest name in string order.
                ("Net-(U1-+)", &["C1.1", "R1.2", "U1.in_p"]),
                ("Net-(U1--)", &["U1.in_n", "U1.out"]),
                ("unconnected-(U1-V+-Pad5)", &["U1.vdd"]),
            ])
        );
        assert!(netlist.warnings.is_empty());
    }

    #[test]
    fn test_unnamed_net_names_follow_kicad() {
        let mut circuit = CircuitBuilder::<()>::new();
        let r: Vec<_> = (0..10)
            .map(|_| circuit.instantiate_part(Box::new(Resistor("1k"))))
            .collect();
        let named = |names: [&str; 2]| {
            let pin = |pad: &str, name| {
                (
                    pad.to_string(),
                    KicadPinInfo::new(pad, Some(name), PinType::Passive),
                )
            };
            let pins = BTreeMap::from([pin("1", names[0]), pin("2", names[1])]);
            CustomPart {
                kicad: Some(KicadPartInfo {
                    pins,
                    ..CustomPart::valid().kicad.unwrap()
                }),
                ..CustomPart::valid()
            }
        };
        let x1 = circuit.instantiate_part(Box::new(named(["1", "2"])));
        let x2 = circuit.instantiate_part(Box::new(named(["A", "B"])));
        circuit.connect(r[8]["1"], r[9]["1"]);
        circuit.connect(r[0]["2"], x1["1"]);
        circuit.connect(r[1]["2"], x2["1"]);
        let netlist = circuit.build(&()).to_netlist().unwrap();
        let name_of = |node: &str| {
            summary(&netlist)
                .into_iter()
                .find(|(_, nodes)| nodes.iter().any(|n| n == node))
                .unwrap()
                .0
        };
        // String order, not natural order (as KiCad does).
        assert_eq!(name_of("R9.1"), "Net-(R10-Pad1)");
        // A pin named as its pad number is written as the pad, like an unnamed pin.
        assert_eq!(name_of("X1.1"), "Net-(R1-Pad2)");
        // A named pin is preferred to unnamed ones.
        assert_eq!(name_of("X2.1"), "Net-(X2-A)");
    }

    #[derive(Debug, Clone)]
    struct Divider;
    impl SubcircuitGenerator for Divider {
        type Options = ();

        // Node 0: input (labeled "IN" inside), node 1: output.
        fn construct_subcircuit(&self, _options: &()) -> CircuitBuilder<()> {
            let mut circuit = CircuitBuilder::new();
            let input = circuit.add_node();
            let output = circuit.add_node();
            circuit.label(input, NetLabel::local("IN"));
            let top = circuit.instantiate_part_named("top", Box::new(Resistor("10k")));
            let bottom = circuit.instantiate_part_named("bottom", Box::new(Resistor("10k")));
            circuit.connect(input, top["1"]);
            circuit.connect(top["2"], output);
            circuit.connect(output, bottom["1"]);
            circuit.label(output, NetLabel::local("MID"));
            circuit.label(bottom["2"], NetLabel::global("GND"));
            circuit
        }
    }

    #[test]
    fn test_hierarchical_names() {
        let mut circuit = CircuitBuilder::<()>::new();
        let interface = HashSet::from([NodeIndex::new(0), NodeIndex::new(1)]);
        let first = circuit.instantiate_subcircuit_named("first", Divider, &interface, |o: &()| *o);
        let second =
            circuit.instantiate_subcircuit_named("second", Divider, &interface, |o: &()| *o);
        circuit.connect(first[&NodeIndex::new(1)], second[&NodeIndex::new(0)]);
        // A name in the outer circuit is preferred to one inside the subcircuit.
        circuit.label(first[&NodeIndex::new(0)], NetLabel::local("ZZZ"));
        let netlist = circuit.build(&()).to_netlist().unwrap();

        assert_eq!(
            summary(&netlist),
            expected(&[
                // Natural order is case-sensitive.
                ("/ZZZ", &["R1.1"]),
                ("/first/MID", &["R1.2", "R2.1", "R3.1"]),
                ("/second/MID", &["R3.2", "R4.1"]),
                ("GND", &["R2.2", "R4.2"]),
            ])
        );
        let paths: Vec<_> = netlist
            .components
            .iter()
            .map(|c| c.path.to_string())
            .collect();
        assert_eq!(paths, vec!["/first/", "/first/", "/second/", "/second/"]);

        let warnings: Vec<_> = netlist.warnings.iter().map(|w| w.kind.clone()).collect();
        assert_eq!(
            warnings,
            vec![
                CircuitWarningKind::MultipleNetNames {
                    used: "/ZZZ".into(),
                    others: vec!["/first/IN".into()],
                },
                CircuitWarningKind::MultipleNetNames {
                    used: "/first/MID".into(),
                    others: vec!["/second/IN".into()],
                },
            ]
        );
        assert_eq!(netlist.warnings[0].notes.len(), 1);
    }

    #[test]
    fn test_label_only_net_is_warned() {
        let mut circuit = CircuitBuilder::<()>::new();
        circuit.instantiate_part(Box::new(Resistor("1k")));
        let node = circuit.add_node();
        circuit.label(node, NetLabel::global("VBUS"));
        let netlist = circuit.build(&()).to_netlist().unwrap();
        assert_eq!(netlist.nets.len(), 2);
        assert_eq!(
            netlist
                .warnings
                .iter()
                .map(|w| w.kind.clone())
                .collect::<Vec<_>>(),
            vec![CircuitWarningKind::LabelOnlyNet {
                names: vec!["VBUS".into()]
            }]
        );
    }

    #[test]
    fn test_parts_without_kicad_info_use_pin_ids() {
        let mut circuit = CircuitBuilder::<()>::new();
        let v = circuit.instantiate_part(IndependentSource::voltage(5.0));
        let r = circuit.instantiate_part(Box::new(Resistor("1k")));
        circuit.connect(v["p"], r["1"]);
        circuit.connect(v["n"], r["2"]);
        let v2 = circuit.instantiate_part(IndependentSource::voltage(5.0));
        circuit.connect(v2["p"], v2["n"]);
        let netlist = circuit.build(&()).to_netlist().unwrap();
        assert_eq!(
            summary(&netlist),
            expected(&[
                ("Net-(R1-Pad1)", &["R1.1", "V1.p"]),
                ("Net-(R1-Pad2)", &["R1.2", "V1.n"]),
                ("Net-(V2-n)", &["V2.n", "V2.p"]),
            ])
        );
    }

    #[test]
    fn test_errors_are_collected() {
        let mut circuit = CircuitBuilder::<()>::new();
        let line1 = line!() + 1;
        let a = circuit.instantiate_part(Box::new(CustomPart {
            pin_ids: vec!["1", "1", "2"],
            ..CustomPart::valid()
        }));
        let line2 = line!() + 1;
        circuit.instantiate_part(Box::new(CustomPart {
            internal_connections: vec![vec!["1", "3"]],
            ..CustomPart::valid()
        }));
        // Collides with the name of the unnamed net of a["2"].
        let line3 = line!() + 1;
        circuit.label(a["1"], NetLabel::global("unconnected-(X1-Pad2)"));
        let errors = circuit.build(&()).to_netlist().unwrap_err().0;

        let kinds: Vec<_> = errors.iter().map(|e| e.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                CircuitErrorKind::DuplicatePinId {
                    ref_name: "X1".into(),
                    pin_id: "1".into()
                },
                CircuitErrorKind::UnknownPinId {
                    ref_name: "X2".into(),
                    pin_id: "3".into()
                },
                CircuitErrorKind::DuplicateNetName {
                    name: "unconnected-(X1-Pad2)".into()
                },
            ]
        );
        let lines: Vec<_> = errors
            .iter()
            .map(|e| e.trace.innermost().unwrap().line())
            .collect();
        assert_eq!(lines[..2], [line1, line2]);
        assert!(lines[2] == line3 || errors[2].notes[0].trace.innermost().unwrap().line() == line3);
    }

    #[test]
    fn test_internal_connections_and_escaped_pin_names() {
        let mut circuit = CircuitBuilder::<()>::new();
        circuit.instantiate_part(Box::new(MultiGndIc));
        let netlist = circuit.build(&()).to_netlist().unwrap();
        assert_eq!(
            summary(&netlist),
            expected(&[
                ("Net-(U1-GND-Pad10)", &["U1.gnd_a", "U1.gnd_b", "U1.gnd_c"]),
                ("unconnected-(U1-I{slash}O-Pad3)", &["U1.io"]),
                ("unconnected-(U1-VCC-Pad1)", &["U1.vcc"]),
            ])
        );
    }
}
