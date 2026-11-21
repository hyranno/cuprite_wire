//! SPICE netlist (deck body) export.
//!
//! See [SPICE Netlist Format](https://github.com/hyranno/cuprite_wire/blob/main/doc/dev/spice_netlist_format.md) for the format.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, btree_map::Entry},
    fmt, io,
    path::PathBuf,
};

use crate::{
    error::{CircuitError, CircuitErrorKind, CircuitErrors},
    naming::{case_insensitive_duplicates, unescape_name},
    netlist::{Component, Netlist},
    spice::{ModelSource, SpiceDeck, SpiceDialect, SpicePartInfo, SpiceProblem, spice_path},
    trace::Trace,
};

/// The node name of the ground.
pub const GROUND: &str = "0";

/// A netlist checked and resolved for SPICE. Write it with [`SpiceNetlist::write`] or `Display`,
/// or add analyses with [`SpiceNetlist::deck`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpiceNetlist {
    title: String,
    includes: BTreeSet<PathBuf>,
    /// Dialects of the models, with whether the model is inline (selecting ngspice's compatibility mode).
    dialects: BTreeSet<(SpiceDialect, bool)>,
    /// Inline model definitions, sorted by name (case-insensitively).
    models: Vec<String>,
    elements: Vec<SpiceLine>,
    /// SPICE node name of each net used by an element, keyed by the net name.
    nodes: BTreeMap<String, String>,
    #[cfg(feature = "shared-ngspice")]
    cosim_devices: Vec<crate::spice::cosim::CosimInstance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpiceLine {
    name: String,
    nodes: Vec<String>,
    args: String,
}

/// An element line whose nodes are still net indices.
struct PendingLine<'a> {
    name: String,
    nets: Vec<usize>,
    args: String,
    /// The trace of the component.
    trace: &'a Trace,
    #[cfg(feature = "shared-ngspice")]
    cosim_device: Option<crate::spice::cosim::CosimDevice>,
}

/// The models used by the elements.
#[derive(Default)]
struct Models<'a> {
    includes: BTreeSet<PathBuf>,
    dialects: BTreeSet<(SpiceDialect, bool)>,
    /// Lowercase name -> (inline definition, trace of the first component using it).
    inline: BTreeMap<String, (String, &'a Trace)>,
}

impl<'a> Models<'a> {
    /// Adds a model used by the component at `trace`. Reports an inline model defined
    /// differently by another component.
    fn add(&mut self, model: &ModelSource, trace: &'a Trace, errors: &mut Vec<CircuitError>) {
        self.dialects
            .insert((model.dialect(), matches!(model, ModelSource::Inline { .. })));
        match model {
            ModelSource::Include { path, .. } => {
                self.includes.insert(path.clone());
            }
            ModelSource::Inline { name, text, .. } => {
                let text = text.trim_end().to_string();
                match self.inline.entry(name.to_ascii_lowercase()) {
                    Entry::Vacant(entry) => {
                        entry.insert((text, trace));
                    }
                    Entry::Occupied(entry) if entry.get().0 != text => {
                        errors.push(
                            CircuitError::new(
                                CircuitErrorKind::SpiceModelConflict { name: name.clone() },
                                trace.clone(),
                            )
                            .with_note(
                                "first defined by the part instantiated here",
                                entry.get().1.clone(),
                            ),
                        );
                    }
                    Entry::Occupied(_) => {}
                }
            }
        }
    }
}

/// Writes the line of an element: its name, its nodes and the arguments, if any.
pub(crate) fn write_element_line(
    f: &mut impl fmt::Write,
    name: &str,
    nodes: &[String],
    args: &str,
) -> fmt::Result {
    write!(f, "{name}")?;
    for node in nodes {
        write!(f, " {node}")?;
    }
    if !args.is_empty() {
        write!(f, " {args}")?;
    }
    writeln!(f)
}

/// Converts a net name to a SPICE node name like KiCad: escapes are undone,
/// `% ( ) , [ ] < > ~` and whitespace become `_` (`Net-(R1-Pad2)` -> `Net-_R1-Pad2_`),
/// a leading `//` (a comment in ngspice) becomes `/root/`, and the ground net (`GND` in any case, or `0`)
/// becomes [`GROUND`].
pub fn spice_node_name(net_name: &str) -> String {
    let name: String = unescape_name(net_name)
        .chars()
        .map(|c| match c {
            '%' | '(' | ')' | ',' | '[' | ']' | '<' | '>' | '~' => '_',
            c if c.is_whitespace() => '_',
            c => c,
        })
        .collect();
    if name == GROUND || name.eq_ignore_ascii_case("GND") {
        GROUND.to_string()
    } else if let Some(rest) = name.strip_prefix("//") {
        format!("/root/{rest}")
    } else {
        name
    }
}

/// Why a converted node name cannot be used, found by testing ngspice 44.2.
fn check_node_name(node: &str) -> Option<String> {
    if node.is_empty() {
        return Some("the name is empty".into());
    }
    if let Some(c) = node.chars().find(|c| !c.is_ascii()) {
        return Some(format!(
            "{c:?} is not ASCII (ngspice replaces it, which can merge nets)"
        ));
    }
    if let Some(c) = node.chars().find(|c| c.is_ascii_control()) {
        return Some(format!("it contains the control character {c:?}"));
    }
    if let Some(c) = node
        .chars()
        .find(|c| matches!(c, '=' | '{' | '}' | '\'' | '"' | ';'))
    {
        return Some(format!("ngspice cannot parse {c:?} in a node name"));
    }
    // A `$` after whitespace starts a comment. A leading `*` or `+` only matters at the start of a line,
    // where node names never are.
    if node.starts_with('$') {
        return Some("a node name cannot start with '$'".into());
    }
    None
}

/// The element name: the device letter followed by the reference, unless the reference
/// already starts with it (case-sensitively, like KiCad), and the suffix.
fn element_name(device: char, ref_name: &str, suffix: &str) -> String {
    if ref_name.starts_with(device) {
        format!("{ref_name}{suffix}")
    } else {
        format!("{device}{ref_name}{suffix}")
    }
}

impl Netlist {
    /// Resolves the SPICE information of every component and checks it.
    ///
    /// # Errors
    /// The problems found in the SPICE information (a part without SPICE information, an unused
    /// pin not declared unused, colliding node or element names, conflicting models, no ground,
    /// ...). All errors are collected, each with the trace of the
    /// component or the net.
    #[expect(
        clippy::missing_panics_doc,
        reason = "panics only if the netlist breaks its invariant that every pin is on a net"
    )]
    pub fn to_spice(&self) -> Result<SpiceNetlist, CircuitErrors> {
        let mut errors = Vec::new();
        let net_of_pin: HashMap<(usize, &str), usize> = self
            .nets
            .iter()
            .enumerate()
            .flat_map(|(net, n)| {
                n.nodes
                    .iter()
                    .map(move |node| ((node.component, node.pin_id.as_str()), net))
            })
            .collect();

        let mut lines = Vec::new();
        let mut models = Models::default();
        for (index, component) in self.components.iter().enumerate() {
            let Some(SpicePartInfo::Elements { elements, .. }) =
                check_component(component, &mut errors)
            else {
                continue;
            };
            for element in elements {
                for model in &element.models {
                    models.add(model, &component.trace, &mut errors);
                }
                let nets = element
                    .nodes
                    .iter()
                    .map(|pin_id| {
                        *net_of_pin
                            .get(&(index, pin_id.as_str()))
                            .expect("every pin of a component belongs to a net")
                    })
                    .collect();
                lines.push(PendingLine {
                    name: element_name(element.device, &component.ref_name, &element.suffix),
                    nets,
                    args: element.args.trim().to_string(),
                    trace: &component.trace,
                    #[cfg(feature = "shared-ngspice")]
                    cosim_device: component.part.cosim_device().cloned(),
                });
            }
        }

        let nodes = self.resolve_nodes(
            lines.iter().flat_map(|line| line.nets.iter().copied()),
            &mut errors,
        );
        check_element_names(
            lines.iter().map(|line| (line.name.as_str(), line.trace)),
            &mut errors,
        );

        #[cfg(feature = "shared-ngspice")]
        let mut cosim_devices = Vec::new();
        let mut elements = Vec::new();
        for line in lines {
            let element = SpiceLine {
                name: line.name,
                nodes: line.nets.iter().map(|net| nodes[net].clone()).collect(),
                args: line.args,
            };
            #[cfg(feature = "shared-ngspice")]
            if let Some(device) = line.cosim_device {
                cosim_devices.push(crate::spice::cosim::CosimInstance {
                    element: element.name.clone(),
                    nodes: element.nodes.clone(),
                    device,
                });
            }
            elements.push(element);
        }
        let nodes = nodes
            .into_iter()
            .map(|(net, node)| (self.nets[net].name.clone(), node))
            .collect();
        CircuitErrors::into_result(
            errors,
            SpiceNetlist {
                title: env!("CARGO_PKG_NAME").to_string(),
                includes: models.includes,
                dialects: models.dialects,
                models: models.inline.into_values().map(|(text, _)| text).collect(),
                elements,
                nodes,
                #[cfg(feature = "shared-ngspice")]
                cosim_devices,
            },
        )
    }

    /// SPICE node names of the nets used by elements, after checking them.
    fn resolve_nodes(
        &self,
        used: impl Iterator<Item = usize>,
        errors: &mut Vec<CircuitError>,
    ) -> BTreeMap<usize, String> {
        let nodes: BTreeMap<usize, String> = used
            .map(|net| (net, spice_node_name(&self.nets[net].name)))
            .collect();
        let nets: Vec<usize> = nodes.keys().copied().collect();
        let collisions: HashMap<usize, usize> =
            case_insensitive_duplicates(nodes.values().map(String::as_str))
                .into_iter()
                .collect();
        for (i, (&net, node)) in nodes.iter().enumerate() {
            let name = &self.nets[net].name;
            let trace = &self.nets[net].trace;
            if let Some(reason) = check_node_name(node) {
                errors.push(CircuitError::new(
                    CircuitErrorKind::InvalidSpiceNodeName {
                        name: name.clone(),
                        reason,
                    },
                    trace.clone(),
                ));
            }
            if let Some(&first) = collisions.get(&i) {
                let other = &self.nets[nets[first]];
                errors.push(
                    CircuitError::new(
                        CircuitErrorKind::SpiceNodeCollision {
                            name: name.clone(),
                            other: other.name.clone(),
                            node: node.clone(),
                        },
                        trace.clone(),
                    )
                    .with_note("the other net is named here", other.trace.clone()),
                );
            }
        }
        if !nodes.values().any(|node| node == GROUND) {
            errors.push(CircuitError::new(
                CircuitErrorKind::GroundMissing,
                Trace::default(),
            ));
        }
        nodes
    }
}

/// Returns the SPICE information of a component if it can be exported.
fn check_component(component: &Component, errors: &mut Vec<CircuitError>) -> Option<SpicePartInfo> {
    let check = |info: &SpicePartInfo| {
        let problems = info.check(&component.pins);
        problems
            .into_iter()
            .map(|problem| problem_kind(problem, component.ref_name.clone()))
            .collect()
    };
    component.checked_info(
        component.part.spice(),
        |ref_name| CircuitErrorKind::SpiceUnsupported { ref_name },
        check,
        errors,
    )
}

fn problem_kind(problem: SpiceProblem, ref_name: String) -> CircuitErrorKind {
    match problem {
        SpiceProblem::NoElements => CircuitErrorKind::InvalidSpiceElement {
            ref_name,
            reason: "no SPICE elements (use `SpicePartInfo::Excluded` to leave the part out)"
                .into(),
        },
        SpiceProblem::InvalidElement { suffix, reason } => CircuitErrorKind::InvalidSpiceElement {
            ref_name,
            reason: if suffix.is_empty() {
                reason
            } else {
                format!("element {suffix:?}: {reason}")
            },
        },
        SpiceProblem::DuplicateSuffix { suffix } => CircuitErrorKind::InvalidSpiceElement {
            ref_name,
            reason: format!("more than one element has the suffix {suffix:?}"),
        },
        SpiceProblem::UnknownPin { pin_id } => CircuitErrorKind::UnknownPinId { ref_name, pin_id },
        SpiceProblem::UnmappedPin { pin_id } => {
            CircuitErrorKind::SpicePinUnmapped { ref_name, pin_id }
        }
    }
}

fn check_element_names<'a>(
    names: impl Iterator<Item = (&'a str, &'a Trace)>,
    errors: &mut Vec<CircuitError>,
) {
    let names: Vec<_> = names.collect();
    for (i, first) in case_insensitive_duplicates(names.iter().map(|&(name, _)| name)) {
        let ((name, trace), (other, other_trace)) = (names[i], names[first]);
        errors.push(
            CircuitError::new(
                CircuitErrorKind::SpiceElementCollision {
                    name: name.to_string(),
                    other: other.to_string(),
                },
                trace.clone(),
            )
            .with_note("the other part is instantiated here", other_trace.clone()),
        );
    }
}

impl SpiceNetlist {
    /// The co-simulated devices, with their elements and nodes.
    #[cfg(feature = "shared-ngspice")]
    pub(crate) fn cosim_devices(&self) -> &[crate::spice::cosim::CosimInstance] {
        &self.cosim_devices
    }

    /// Sets the title line (default: the crate name). Line breaks are replaced by spaces.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into().lines().collect::<Vec<_>>().join(" ");
        self
    }

    /// The SPICE node name of a net, e.g. `"/out"` for the net `/out`, `"0"` for `GND`.
    /// `None` if no element uses the net.
    pub fn node(&self, net_name: &str) -> Option<&str> {
        self.nodes.get(net_name).map(String::as_str)
    }

    /// The value of ngspice's `ngbehavior` the models need (`ps`, `psa`, `lt`, ...; see
    /// [`SpiceDialect::ngbehavior`]), or `None` for the default mode. [`Ngspice::run`](crate::spice::Ngspice::run) sets it; to run the deck
    /// yourself, put `set ngbehavior=<value>` into `.spiceinit` in the working directory
    /// (`.options ngbehavior=` in the deck has no effect).
    pub fn ngbehavior(&self) -> Option<&'static str> {
        SpiceDialect::ngbehavior(self.dialects.iter().copied())
    }

    /// A deck of this netlist, to add analyses and run it.
    pub fn deck(self) -> SpiceDeck {
        SpiceDeck::new(self)
    }

    /// Writes everything but `.end`.
    pub(crate) fn fmt_body(&self, f: &mut impl fmt::Write) -> fmt::Result {
        writeln!(f, ".title {}", self.title)?;
        if let Some(mode) = self.ngbehavior() {
            writeln!(f, "* requires: set ngbehavior={mode} (.spiceinit)")?;
        }
        for path in &self.includes {
            writeln!(f, ".include \"{}\"", spice_path(path))?;
        }
        for model in &self.models {
            writeln!(f, "{model}")?;
        }
        for element in &self.elements {
            write_element_line(f, &element.name, &element.nodes, &element.args)?;
        }
        Ok(())
    }

    /// Writes the netlist (as [`Display`](fmt::Display) formats it, ending with `.end`) to `writer`.
    ///
    /// # Errors
    /// The error of `writer`.
    pub fn write(&self, writer: &mut impl io::Write) -> io::Result<()> {
        write!(writer, "{self}")
    }
}

impl fmt::Display for SpiceNetlist {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_body(f)?;
        writeln!(f, ".end")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spice_node_names() {
        let cases = [
            ("/OUT", "/OUT"),
            ("Net-(R1-Pad2)", "Net-_R1-Pad2_"),
            ("Net-(U1-+)", "Net-_U1-+_"),
            ("unconnected-(U1-NC-Pad7)", "unconnected-_U1-NC-Pad7_"),
            ("/amp/I{slash}O", "/amp/I/O"),
            ("/a b,c[d]<e>~f%", "/a_b_c_d__e__f_"),
            ("{slash}{slash}x", "/root/x"),
            ("GND", "0"),
            ("gnd", "0"),
            ("0", "0"),
            ("/GND", "/GND"),
            ("/stage#1/OUT", "/stage#1/OUT"),
        ];
        for (net, node) in cases {
            assert_eq!(spice_node_name(net), node, "{net}");
        }
    }

    #[test]
    fn test_invalid_node_names() {
        for node in [
            "/é", "/a=b", "/a{b}", "/a'b", "/a\"b", "/a;b", "$a", "/a\tb",
        ] {
            assert!(
                check_node_name(node).is_some(),
                "{node:?} should be rejected"
            );
        }
        for node in [
            "/a$b",
            "/a*b",
            "/a+b",
            "+5V",
            "*a",
            "-5V",
            "/stage#1/OUT",
            "/a:b.c@d",
            "0",
        ] {
            assert_eq!(check_node_name(node), None, "{node:?} should be accepted");
        }
    }

    #[test]
    fn test_element_names() {
        assert_eq!(element_name('R', "R1", ""), "R1");
        assert_eq!(element_name('R', "Rload1", ""), "Rload1");
        assert_eq!(element_name('X', "U1", "A"), "XU1A");
        assert_eq!(element_name('M', "Q2", ""), "MQ2");
        assert_eq!(element_name('R', "r1", ""), "Rr1");
    }
}
