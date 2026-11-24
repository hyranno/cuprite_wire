//! Describing circuits: nodes, parts, labels and subcircuits, built into a netlist.
//!
//! See the guide [Describing circuits](https://github.com/hyranno/cuprite_wire/blob/main/doc/circuits.md).

use std::{
    collections::HashMap,
    fmt::Debug,
    panic::Location,
    sync::atomic::{AtomicU64, Ordering},
};

use petgraph::{
    graph::{NodeIndex, UnGraph},
    unionfind::UnionFind,
    visit::EdgeRef,
};

use crate::{
    erc::{ErcMark, ErcRule, NodeMark},
    error::{CircuitError, CircuitErrorKind},
    naming::{
        AUTO_NAME_SEPARATOR, HierPath, NetLabel, NodeLabel, check_base_name, check_explicit_name,
        check_label_name, check_ref_prefix,
    },
    parts::{Part, PartInstance, PinType},
    subcircuit::{BoundGenerator, Interface, IntoSubcircuit, SubcircuitInstance, interface_nodes},
    trace::{Loc, Trace},
};

/// Weight of a node in the net graph.
#[derive(Debug, Clone)]
pub(crate) struct NodeInfo {
    /// Where the node was created.
    pub(crate) trace: Trace,
    /// Labels attached to the node (including those attached inside subcircuits
    /// when this node is an interface node).
    pub(crate) labels: Vec<NodeLabel>,
    /// ERC marks attached to the node (including those attached inside subcircuits).
    pub(crate) marks: Vec<NodeMark>,
}

impl NodeInfo {
    fn new(trace: Trace) -> Self {
        NodeInfo {
            trace,
            labels: Vec::new(),
            marks: Vec::new(),
        }
    }
}

/// Nodes carry their creation location and labels, and edges carry the location where they were connected.
pub(crate) type NetGraph = UnGraph<NodeInfo, Loc>;

/// Identifies a [`CircuitBuilder`] (its clones share it), so that the nodes of other builders are
/// detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct BuilderId(u64);

impl BuilderId {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        BuilderId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// A node of a circuit: a point that pins and other nodes can be connected to. Created by a
/// [`CircuitBuilder`] and usable only with it; a node of another builder (e.g. of the circuit
/// around a subcircuit, used inside it) is reported as [`CircuitErrorKind::ForeignNode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Node {
    builder: BuilderId,
    index: NodeIndex,
}

impl Node {
    pub(crate) fn index(self) -> NodeIndex {
        self.index
    }
}

/// Assigns instance names in one (sub)circuit: explicit names are checked for validity and
/// uniqueness, and automatic names are numbered per reference prefix or subcircuit base name
/// (`R#1`, `R#2`, ...).
#[derive(Debug, Clone, Default)]
pub(crate) struct NameRegistry {
    counters: HashMap<String, u32>,
    explicit_names: HashMap<String, Trace>,
}

impl NameRegistry {
    fn assign(
        &mut self,
        explicit_name: Option<&str>,
        base_name: &str,
        trace: &Trace,
        errors: &mut Vec<CircuitError>,
    ) -> String {
        let Some(name) = explicit_name else {
            let counter = self.counters.entry(base_name.to_string()).or_insert(0);
            *counter += 1;
            return format!("{base_name}{AUTO_NAME_SEPARATOR}{counter}");
        };
        if let Err(reason) = check_explicit_name(name) {
            errors.push(CircuitError::new(
                CircuitErrorKind::InvalidName {
                    name: name.to_string(),
                    reason,
                },
                trace.clone(),
            ));
        } else if let Some(previous) = self.explicit_names.get(name) {
            errors.push(
                CircuitError::new(
                    CircuitErrorKind::DuplicateInstanceName {
                        name: name.to_string(),
                    },
                    trace.clone(),
                )
                .with_note("the name is first used here", previous.clone()),
            );
        } else {
            self.explicit_names.insert(name.to_string(), trace.clone());
        }
        name.to_string()
    }
}

/// Describes a circuit: nodes, parts, labels, ERC marks and subcircuit instances.
///
/// `B` is the type of the options the circuit is built with ([`CircuitBuilder::build`]), passed
/// to the generators of its subcircuits.
pub struct CircuitBuilder<B = ()> {
    id: BuilderId,
    pub(crate) parts: Vec<PartInstance>,
    pub(crate) subcircuits: Vec<SubcircuitInstance<B>>,
    pub(crate) nets: NetGraph,
    pub(crate) subcircuit_names: NameRegistry,
    pub(crate) part_names: NameRegistry,
    pub(crate) errors: Vec<CircuitError>,
}

impl<B> Clone for CircuitBuilder<B> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            parts: self.parts.clone(),
            subcircuits: self.subcircuits.clone(),
            nets: self.nets.clone(),
            subcircuit_names: self.subcircuit_names.clone(),
            part_names: self.part_names.clone(),
            errors: self.errors.clone(),
        }
    }
}

impl<B> Debug for CircuitBuilder<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBuilder")
            .field("parts", &self.parts)
            .field("subcircuits", &self.subcircuits)
            .field("nets", &self.nets)
            .field("errors", &self.errors)
            .finish_non_exhaustive()
    }
}

impl<B> CircuitBuilder<B> {
    /// An empty circuit.
    pub fn new() -> Self {
        CircuitBuilder {
            id: BuilderId::new(),
            parts: Vec::new(),
            subcircuits: Vec::new(),
            nets: NetGraph::new_undirected(),
            subcircuit_names: NameRegistry::default(),
            part_names: NameRegistry::default(),
            errors: Vec::new(),
        }
    }

    /// Expands all subcircuits. This always succeeds; problems found while describing the
    /// circuit are reported when it is converted to a netlist.
    pub fn build(&self, options: &B) -> BuiltCircuit {
        self.clone().expand(options).into()
    }

    pub(crate) fn expand(self, options: &B) -> ExpandedCircuit {
        let CircuitBuilder {
            id: _,
            parts,
            subcircuits,
            nets,
            subcircuit_names: _,
            part_names: _,
            errors,
        } = self;
        let mut expanded = ExpandedCircuit {
            parts,
            nets,
            errors,
        };

        for instance in subcircuits {
            let (subcircuit, interface) = instance.generator.generate(options, instance.location);
            expanded.merge(subcircuit, &interface, &instance);
        }

        expanded
    }

    /// Creates a new node (a point that pins and other nodes can be connected to).
    #[track_caller]
    pub fn add_node(&mut self) -> Node {
        self.add_node_at(Location::caller())
    }

    pub(crate) fn add_node_at(&mut self, location: &'static Location<'static>) -> Node {
        let index = self.nets.add_node(NodeInfo::new(Trace::new(location)));
        self.node(index)
    }

    fn node(&self, index: NodeIndex) -> Node {
        Node {
            builder: self.id,
            index,
        }
    }

    /// The index of `node` in this circuit, or `None` (reported as
    /// [`CircuitErrorKind::ForeignNode`] at `location`) if another builder created it.
    fn index_of(&mut self, node: Node, location: Loc) -> Option<NodeIndex> {
        if node.builder == self.id {
            return Some(node.index);
        }
        self.errors.push(CircuitError::new(
            CircuitErrorKind::ForeignNode,
            Trace::new(location),
        ));
        None
    }

    /// Connects two nodes.
    #[track_caller]
    pub fn connect(&mut self, a: Node, b: Node) {
        let location = Location::caller();
        if let (Some(a), Some(b)) = (self.index_of(a, location), self.index_of(b, location)) {
            self.nets.add_edge(a, b, location);
        }
    }

    /// Reports `name` as a [`CircuitErrorKind::InvalidName`] if `check` rejects it.
    fn check_name(
        &mut self,
        name: &str,
        check: fn(&str) -> Result<(), &'static str>,
        trace: Trace,
    ) {
        if let Err(reason) = check(name) {
            self.errors.push(CircuitError::new(
                CircuitErrorKind::InvalidName {
                    name: name.to_string(),
                    reason,
                },
                trace,
            ));
        }
    }

    /// Gives a name to the net containing `node`.
    ///
    /// Nodes with the same global label, or with the same local label in the same
    /// (sub)circuit instance, are connected. If a net ends up with several names,
    /// one of them is used (see the netlist conversion) and a warning is reported. A label on a
    /// net without part pins is reported as [`CircuitWarningKind::LabelOnlyNet`].
    ///
    /// [`CircuitWarningKind::LabelOnlyNet`]: crate::error::CircuitWarningKind::LabelOnlyNet
    #[track_caller]
    pub fn label(&mut self, node: Node, label: NetLabel) {
        let location = Location::caller();
        let trace = Trace::new(location);
        self.check_name(label.name(), check_label_name, trace.clone());
        let Some(node) = self.index_of(node, location) else {
            return;
        };
        self.nets[node].labels.push(NodeLabel {
            label,
            scope: HierPath::default(),
            trace,
        });
    }

    /// Marks the net containing `node` as intentionally unconnected (KiCad's no-connect flag).
    /// ERC reports the mark if the net has more than one pin, or no pin.
    #[track_caller]
    pub fn no_connect(&mut self, node: Node) {
        self.mark(node, ErcMark::NoConnect);
    }

    /// Marks the net containing `node` as driven by a power source outside the circuit
    /// (KiCad's `PWR_FLAG`): a virtual `power_out` pin named `PWR_FLAG`.
    #[track_caller]
    pub fn power_flag(&mut self, node: Node) {
        self.virtual_pin(node, PinType::PowerOut, "PWR_FLAG");
    }

    /// Adds a pin outside the circuit to the net containing `node`, seen only by ERC
    /// (e.g. what an off-board connection or the circuit around a subcircuit provides).
    #[track_caller]
    pub fn virtual_pin(&mut self, node: Node, pin_type: PinType, name: &str) {
        self.mark(
            node,
            ErcMark::VirtualPin {
                pin_type,
                name: name.to_string(),
            },
        );
    }

    /// Leaves the violations of `rule` on the net containing `node` out of ERC (like excluding a
    /// KiCad ERC marker), e.g. [`ErcRule::PinToPin`] for outputs that are connected on purpose.
    /// Nothing is reported if there is no such violation. On a net without part pins, it allows
    /// the rules about such nets ([`ErcRule::NoConnectDangling`], [`ErcRule::VirtualPinDangling`]
    /// and [`ErcRule::LabelOnlyNet`]).
    #[track_caller]
    pub fn erc_allow(&mut self, node: Node, rule: ErcRule) {
        self.mark(node, ErcMark::Allow(rule));
    }

    #[track_caller]
    fn mark(&mut self, node: Node, mark: ErcMark) {
        let location = Location::caller();
        if let Some(node) = self.index_of(node, location) {
            let trace = Trace::new(location);
            self.nets[node].marks.push(NodeMark { mark, trace });
        }
    }

    /// Instantiates a subcircuit and returns its interface: new nodes in this circuit, connected
    /// to the subcircuit's interface nodes.
    ///
    /// `subcircuit` is a [`SubcircuitGenerator`], generated with the options this circuit is built
    /// with, or a generator with its own options ([`SubcircuitGenerator::with_options`]). Its
    /// instance name is automatic (`<base name>#<n>`), or explicit with [`Named`]: the name is
    /// used in hierarchical net names (`/name/LABEL`) and as the key of the deterministic UUIDs of
    /// the parts inside it.
    ///
    /// [`SubcircuitGenerator`]: crate::subcircuit::SubcircuitGenerator
    /// [`SubcircuitGenerator::with_options`]: crate::subcircuit::SubcircuitGenerator::with_options
    #[track_caller]
    pub fn instantiate_subcircuit<S: IntoSubcircuit<B>>(&mut self, subcircuit: S) -> S::Interface {
        let location = Location::caller();
        let (name, bound) = subcircuit.into_named_subcircuit();
        if name.is_none() {
            self.check_name(&bound.base_name, check_base_name, Trace::new(location));
        }
        let name = self.subcircuit_names.assign(
            name.as_deref(),
            &bound.base_name,
            &Trace::new(location),
            &mut self.errors,
        );
        let interface = S::Interface::from_fn(|_| self.add_node_at(location));
        self.subcircuits.push(SubcircuitInstance {
            generator: bound.generator,
            interface: interface_nodes(&interface),
            name,
            location,
        });
        interface
    }

    /// Instantiates a part and returns the nodes of its pins, keyed by pin ID.
    ///
    /// `part` is an `impl Part` or a `Box<dyn Part>`. Its instance name, the key of the
    /// deterministic UUID of the part, is automatic (`<reference prefix>#<n>`), or explicit with
    /// [`Named`] (unique in this (sub)circuit). Automatic names shift when a part of the same kind
    /// is added or removed before this one in the same (sub)circuit; name the part to keep the
    /// UUID stable.
    #[track_caller]
    pub fn instantiate_part(&mut self, part: impl IntoPart) -> HashMap<String, Node> {
        let (name, part) = part.into_named_part();
        let trace = Trace::new(Location::caller());
        let prefix = part.ref_prefix();
        self.check_name(&prefix, check_ref_prefix, trace.clone());
        let name = self
            .part_names
            .assign(name.as_deref(), &prefix, &trace, &mut self.errors);
        let mut pin_mapping = HashMap::new();
        for pin in part.pin_ids() {
            pin_mapping.insert(pin, self.nets.add_node(NodeInfo::new(trace.clone())));
        }
        // Unknown pin IDs are skipped here and reported when the netlist is created.
        for group in part.internal_connections() {
            let nodes: Vec<_> = group
                .iter()
                .filter_map(|pin| pin_mapping.get(pin))
                .collect();
            for pair in nodes.windows(2) {
                self.nets.add_edge(*pair[0], *pair[1], trace.0[0]);
            }
        }
        // Unknown pin IDs are skipped here too.
        for (pin, name) in part.global_labels() {
            self.check_name(&name, check_label_name, trace.clone());
            if let Some(&node) = pin_mapping.get(&pin) {
                self.nets[node].labels.push(NodeLabel {
                    label: NetLabel::Global(name),
                    scope: HierPath::default(),
                    trace: trace.clone(),
                });
            }
        }
        let nodes = pin_mapping
            .iter()
            .map(|(pin, &index)| (pin.clone(), self.node(index)))
            .collect();
        self.parts.push(PartInstance {
            part,
            pin_mapping,
            name,
            path: HierPath::default(),
            trace,
        });
        nodes
    }
}

/// An explicit instance name for [`CircuitBuilder::instantiate_part`] and
/// [`CircuitBuilder::instantiate_subcircuit`], e.g. `Named("R_LOAD", part)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Named<'a, T>(pub &'a str, pub T);

/// What [`CircuitBuilder::instantiate_part`] takes: an `impl Part` or a `Box<dyn Part>`,
/// optionally [`Named`].
pub trait IntoPart {
    /// The explicit instance name, if any, and the part.
    #[doc(hidden)]
    fn into_named_part(self) -> (Option<String>, Box<dyn Part>);
}

impl<P: Part + 'static> IntoPart for P {
    fn into_named_part(self) -> (Option<String>, Box<dyn Part>) {
        (None, Box::new(self))
    }
}

impl IntoPart for Box<dyn Part> {
    fn into_named_part(self) -> (Option<String>, Box<dyn Part>) {
        (None, self)
    }
}

impl<P: IntoPart> IntoPart for Named<'_, P> {
    fn into_named_part(self) -> (Option<String>, Box<dyn Part>) {
        (Some(self.0.to_string()), self.1.into_named_part().1)
    }
}

impl<B, S: IntoSubcircuit<B>> IntoSubcircuit<B> for Named<'_, S> {
    type Interface = S::Interface;

    fn into_named_subcircuit(self) -> (Option<String>, BoundGenerator<B>) {
        (Some(self.0.to_string()), self.1.into_named_subcircuit().1)
    }
}

impl<B> Default for CircuitBuilder<B> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExpandedCircuit {
    parts: Vec<PartInstance>,
    nets: NetGraph,
    errors: Vec<CircuitError>,
}

impl ExpandedCircuit {
    /// Merges an expanded subcircuit generated for `instance`, whose interface nodes are
    /// `interface` (in the order of `instance.interface`).
    fn merge<B>(
        &mut self,
        other: ExpandedCircuit,
        interface: &[NodeIndex],
        instance: &SubcircuitInstance<B>,
    ) {
        let location = instance.location;
        let name = instance.name.as_str();
        let outer_label = |label: &NodeLabel| NodeLabel {
            label: label.label.clone(),
            scope: label.scope.prepended(name),
            trace: label.trace.prepended(location),
        };

        let outer_node: HashMap<NodeIndex, NodeIndex> = interface
            .iter()
            .copied()
            .zip(instance.interface.iter().copied())
            .collect();
        let mut index_map = HashMap::<NodeIndex, NodeIndex>::new();
        for node in other.nets.node_indices() {
            let inner = &other.nets[node];
            let labels = inner.labels.iter().map(outer_label);
            let marks = inner.marks.iter().map(|mark| NodeMark {
                mark: mark.mark.clone(),
                trace: mark.trace.prepended(location),
            });
            let merged_node = match outer_node.get(&node) {
                // The interface node keeps its own trace and collects the labels and marks from both sides.
                Some(&merged_node) => {
                    self.nets[merged_node].labels.extend(labels);
                    self.nets[merged_node].marks.extend(marks);
                    merged_node
                }
                None => self.nets.add_node(NodeInfo {
                    trace: inner.trace.prepended(location),
                    labels: labels.collect(),
                    marks: marks.collect(),
                }),
            };
            index_map.insert(node, merged_node);
        }

        for edge in other.nets.edge_indices() {
            let (source, target) = other.nets.edge_endpoints(edge).unwrap();
            self.nets
                .add_edge(index_map[&source], index_map[&target], other.nets[edge]);
        }

        for part in other.parts {
            let pin_mapping = part
                .pin_mapping
                .iter()
                .map(|(pin, node)| (pin.clone(), index_map[node]))
                .collect();
            self.parts.push(PartInstance {
                pin_mapping,
                name: part.name,
                path: part.path.prepended(name),
                trace: part.trace.prepended(location),
                part: part.part,
            });
        }

        self.errors.extend(
            other
                .errors
                .into_iter()
                .map(|error| error.prepended(location)),
        );
    }
}

impl From<ExpandedCircuit> for BuiltCircuit {
    /// Numbers the references of each prefix in the order of the parts.
    fn from(expanded: ExpandedCircuit) -> Self {
        let mut counters = HashMap::<String, u32>::new();
        let ref_names = expanded
            .parts
            .iter()
            .map(|part| {
                let prefix = part.part.ref_prefix();
                let counter = counters.entry(prefix.clone()).or_insert(0);
                *counter += 1;
                format!("{prefix}{counter}")
            })
            .collect();
        BuiltCircuit {
            parts: expanded.parts,
            ref_names,
            nets: expanded.nets,
            errors: expanded.errors,
        }
    }
}

/// A circuit with all subcircuits expanded, returned by [`CircuitBuilder::build`]. Convert it
/// with [`BuiltCircuit::to_netlist`].
#[derive(Debug, Clone)]
pub struct BuiltCircuit {
    pub(crate) parts: Vec<PartInstance>,
    /// The reference designator of each part.
    pub(crate) ref_names: Vec<String>,
    pub(crate) nets: NetGraph,
    /// Errors found while describing the circuit (reported by the netlist conversion).
    pub(crate) errors: Vec<CircuitError>,
}

impl BuiltCircuit {
    /// The errors found while describing the circuit, reported by [`BuiltCircuit::to_netlist`].
    pub fn errors(&self) -> &[CircuitError] {
        &self.errors
    }

    /// Groups nodes into nets: nodes connected by edges, by the same global label,
    /// or by the same local label in the same subcircuit instance.
    /// Each group is sorted, and groups are sorted by their first node.
    pub(crate) fn connected_nodes(&self) -> Vec<Vec<NodeIndex>> {
        let mut union_find = UnionFind::<usize>::new(self.nets.node_count());
        for edge in self.nets.edge_references() {
            union_find.union(edge.source().index(), edge.target().index());
        }
        let mut first_node_by_name = HashMap::<(bool, String), NodeIndex>::new();
        for node in self.nets.node_indices() {
            for label in &self.nets[node].labels {
                let key = (matches!(label.label, NetLabel::Global(_)), label.net_name());
                let first = *first_node_by_name.entry(key).or_insert(node);
                union_find.union(first.index(), node.index());
            }
        }

        let mut groups = HashMap::<usize, Vec<NodeIndex>>::new();
        for node in self.nets.node_indices() {
            groups
                .entry(union_find.find(node.index()))
                .or_default()
                .push(node);
        }
        let mut groups: Vec<_> = groups.into_values().collect();
        groups.sort();
        groups
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;
    use petgraph::graph::NodeIndex;
    use std::collections::{HashMap, HashSet};

    use crate::{parts::Part, subcircuit::SubcircuitGenerator};

    use super::*;

    /// `(reference, pin ID)` of the part pin at each node.
    fn pins_by_node(circuit: &BuiltCircuit) -> HashMap<NodeIndex, (String, String)> {
        circuit
            .parts
            .iter()
            .zip(&circuit.ref_names)
            .flat_map(|(part, ref_name)| {
                part.pin_mapping
                    .iter()
                    .map(|(pin_id, &node)| (node, (ref_name.clone(), pin_id.clone())))
            })
            .collect()
    }

    /// The part pins (`U1.pin1`) of each net with any, sorted.
    fn connected_pin_names(circuit: &BuiltCircuit) -> Vec<Vec<String>> {
        let pins = pins_by_node(circuit);
        circuit
            .connected_nodes()
            .into_iter()
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|node| pins.get(node))
                    .map(|(ref_name, pin_id)| format!("{ref_name}.{pin_id}"))
                    .sorted()
                    .collect::<Vec<_>>()
            })
            .filter(|pins| !pins.is_empty())
            .sorted()
            .collect()
    }

    /// The paths of the parts, in order.
    fn paths(circuit: &BuiltCircuit) -> Vec<String> {
        circuit
            .parts
            .iter()
            .map(|part| part.path.to_string())
            .collect()
    }

    /// The names reported as invalid, in order.
    fn invalid_names(circuit: &BuiltCircuit) -> Vec<String> {
        circuit
            .errors()
            .iter()
            .filter_map(|error| match &error.kind {
                CircuitErrorKind::InvalidName { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// The lines of the locations of `trace`.
    fn lines(trace: &Trace) -> Vec<u32> {
        trace.0.iter().map(|loc| loc.line()).collect()
    }

    #[derive(Debug, Clone)]
    struct TestPart;
    struct TestPartPins {
        pub pin1: Node,
        pub pin2: Node,
    }
    impl Part for TestPart {
        fn ref_prefix(&self) -> String {
            "U".to_string()
        }

        fn pin_ids(&self) -> Vec<String> {
            vec!["pin1".to_string(), "pin2".to_string()]
        }

        fn value(&self) -> String {
            "test".to_string()
        }
    }
    impl TestPart {
        #[track_caller]
        fn instantiate(circuit: &mut CircuitBuilder) -> TestPartPins {
            let pins = circuit.instantiate_part(TestPart);
            TestPartPins {
                pin1: pins["pin1"],
                pin2: pins["pin2"],
            }
        }
    }

    // Subcircuit containing two parts connected in parallel
    #[derive(Debug, Clone)]
    struct TestSubcircuitGenerator1;
    struct TestSubcircuitGenerator1Pins {
        pub pin1: Node,
        pub pin2: Node,
    }
    impl SubcircuitGenerator for TestSubcircuitGenerator1 {
        type Options = ();
        type Interface = [Node; 2];

        fn construct_subcircuit(
            &self,
            _options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            interface: Self::Interface,
        ) {
            let [node1, node2] = interface;
            let part1_pins = TestPart::instantiate(circuit);
            let part2_pins = TestPart::instantiate(circuit);

            circuit.connect(node1, part1_pins.pin1);
            circuit.connect(part1_pins.pin2, node2);
            circuit.connect(node1, part2_pins.pin1);
            circuit.connect(part2_pins.pin2, node2);
        }
    }
    impl TestSubcircuitGenerator1 {
        #[track_caller]
        fn instantiate(circuit: &mut CircuitBuilder) -> TestSubcircuitGenerator1Pins {
            let [pin1, pin2] = circuit.instantiate_subcircuit(TestSubcircuitGenerator1);
            TestSubcircuitGenerator1Pins { pin1, pin2 }
        }
    }

    // Subcircuit containing a part and an instance of the first subcircuit connected in series
    #[derive(Debug, Clone)]
    struct TestSubcircuitGenerator2;
    #[allow(dead_code)]
    struct TestSubcircuitGenerator2Pins {
        pub pin1: Node,
        pub pin2: Node,
    }
    impl SubcircuitGenerator for TestSubcircuitGenerator2 {
        type Options = ();
        type Interface = [Node; 2];

        fn construct_subcircuit(
            &self,
            _options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            interface: Self::Interface,
        ) {
            let [node1, node2] = interface;
            let subcircuit_pins = TestSubcircuitGenerator1::instantiate(circuit);
            let part_pins = TestPart::instantiate(circuit);
            circuit.connect(node1, subcircuit_pins.pin1);
            circuit.connect(subcircuit_pins.pin2, part_pins.pin1);
            circuit.connect(part_pins.pin2, node2);
        }
    }
    impl TestSubcircuitGenerator2 {
        #[track_caller]
        fn instantiate(circuit: &mut CircuitBuilder) -> TestSubcircuitGenerator2Pins {
            let [pin1, pin2] = circuit.instantiate_subcircuit(TestSubcircuitGenerator2);
            TestSubcircuitGenerator2Pins { pin1, pin2 }
        }
    }

    #[derive(Debug)]
    struct BuildOptions {
        part_count: usize,
    }

    #[derive(Debug)]
    struct NestedOptions {
        part_count: usize,
    }

    #[derive(Debug)]
    struct LeafOptions {
        part_count: usize,
    }

    #[derive(Debug, Clone)]
    struct NestedOptionsGenerator;

    impl SubcircuitGenerator for NestedOptionsGenerator {
        type Options = NestedOptions;
        type Interface = [Node; 2];

        fn construct_subcircuit(
            &self,
            _options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            interface: Self::Interface,
        ) {
            let [first, second] = interface;
            let [inner_first, inner_second] = circuit.instantiate_subcircuit(
                LeafOptionsGenerator.with_options(|options: &NestedOptions| LeafOptions {
                    part_count: options.part_count,
                }),
            );
            circuit.connect(first, inner_first);
            circuit.connect(second, inner_second);
        }
    }

    #[derive(Debug, Clone)]
    struct LeafOptionsGenerator;

    impl SubcircuitGenerator for LeafOptionsGenerator {
        type Options = LeafOptions;
        type Interface = [Node; 2];

        fn construct_subcircuit(
            &self,
            options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            _interface: Self::Interface,
        ) {
            for _ in 0..options.part_count {
                circuit.instantiate_part(TestPart);
            }
        }
    }

    #[test]
    fn test_generator_specific_options_expand_recursively() {
        let mut circuit = CircuitBuilder::<BuildOptions>::new();
        circuit.instantiate_subcircuit(NestedOptionsGenerator.with_options(
            |options: &BuildOptions| NestedOptions {
                part_count: options.part_count,
            },
        ));

        let one_part = circuit.build(&BuildOptions { part_count: 1 });
        let three_parts = circuit.build(&BuildOptions { part_count: 3 });

        assert_eq!(one_part.parts.len(), 1);
        assert_eq!(three_parts.parts.len(), 3);
        assert_eq!(circuit.subcircuits.len(), 1);
    }

    #[test]
    fn test_expand_subcircuits() {
        let mut circuit = CircuitBuilder::new();
        let _subcircuit_pins2 = TestSubcircuitGenerator2::instantiate(&mut circuit);
        // Before expansion: only the 2 outer nodes of the `TestSubcircuitGenerator2` instance.
        assert_eq!(circuit.nets.node_count(), 2);
        let circuit = circuit.build(&());
        // 1 part inside `TestSubcircuitGenerator2` and 2 inside its `TestSubcircuitGenerator1`.
        assert_eq!(circuit.parts.len(), 3);
        // Nodes: the 2 outer ones; 4 inside `TestSubcircuitGenerator2` (the outer nodes of its
        // `TestSubcircuitGenerator1` instance and the pins of its part); 4 inside
        // `TestSubcircuitGenerator1` (the pins of its two parts). Interface nodes are not copied.
        assert_eq!(circuit.nets.node_count(), 2 + 4 + 4);
        // Edges: the 3 connections in `TestSubcircuitGenerator2` and the 4 in
        // `TestSubcircuitGenerator1`.
        assert_eq!(circuit.nets.edge_count(), 3 + 4);
    }

    #[test]
    fn test_pins_by_node() {
        let mut circuit = CircuitBuilder::new();
        let _subcircuit_pins2 = TestSubcircuitGenerator2::instantiate(&mut circuit);
        let circuit = circuit.build(&());

        let expected: HashMap<_, _> = [
            (4, "U1", "pin1"),
            (5, "U1", "pin2"),
            (6, "U2", "pin1"),
            (7, "U2", "pin2"),
            (8, "U3", "pin1"),
            (9, "U3", "pin2"),
        ]
        .into_iter()
        .map(|(node, ref_name, pin_id)| {
            (
                NodeIndex::new(node),
                (ref_name.to_string(), pin_id.to_string()),
            )
        })
        .collect();
        assert_eq!(pins_by_node(&circuit), expected);
    }

    #[test]
    fn test_refs_are_numbered_in_the_order_of_the_parts() {
        let mut circuit = CircuitBuilder::new();
        TestPart::instantiate(&mut circuit);
        let _subcircuit_pins = TestSubcircuitGenerator2::instantiate(&mut circuit);
        circuit.instantiate_part(crate::test_parts::CustomPart {
            ref_prefix: "R",
            ..crate::test_parts::CustomPart::valid()
        });
        TestPart::instantiate(&mut circuit);

        let circuit = circuit.build(&());

        // The parts of the top level come first, then those of each subcircuit instance.
        assert_eq!(circuit.ref_names, ["U1", "R1", "U2", "U3", "U4", "U5"]);
    }

    #[test]
    fn test_connected_nodes() {
        let mut circuit = CircuitBuilder::new();
        let _subcircuit_pins2 = TestSubcircuitGenerator2::instantiate(&mut circuit);
        let circuit = circuit.build(&());

        assert_eq!(
            connected_pin_names(&circuit),
            vec![
                vec!["U1.pin1", "U2.pin2", "U3.pin2"],
                vec!["U1.pin2"],
                vec!["U2.pin1", "U3.pin1"],
            ]
        );
    }

    #[derive(Debug, Clone)]
    struct TracedSubcircuitGenerator;
    static TRACED_PART_LINE: std::sync::Mutex<u32> = std::sync::Mutex::new(0);
    impl SubcircuitGenerator for TracedSubcircuitGenerator {
        type Options = ();
        type Interface = ();

        fn construct_subcircuit(
            &self,
            _options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            _interface: Self::Interface,
        ) {
            let line = line!() + 1;
            circuit.instantiate_part(TestPart);
            *TRACED_PART_LINE.lock().unwrap() = line;
        }
    }

    #[test]
    fn test_traces_follow_subcircuit_instantiations() {
        let mut circuit = CircuitBuilder::new();
        let part_line = line!() + 1;
        let _ = TestPart::instantiate(&mut circuit);
        let subcircuit_line = line!() + 1;
        circuit.instantiate_subcircuit(TracedSubcircuitGenerator);
        let node_line = line!() + 1;
        let node = circuit.add_node();
        let circuit = circuit.build(&());

        assert!(
            circuit
                .parts
                .iter()
                .all(|part| part.trace.0.iter().all(|loc| loc.file() == file!()))
        );
        assert_eq!(lines(&circuit.parts[0].trace), vec![part_line]);
        assert_eq!(
            lines(&circuit.parts[1].trace),
            vec![subcircuit_line, *TRACED_PART_LINE.lock().unwrap()]
        );
        assert_eq!(lines(&circuit.nets[node.index()].trace), vec![node_line]);
        // Pin nodes of a part inside the subcircuit carry the subcircuit trace too.
        let inner_pin = circuit.parts[1].pin_mapping["pin1"];
        assert_eq!(
            lines(&circuit.nets[inner_pin].trace),
            lines(&circuit.parts[1].trace)
        );
    }

    #[derive(Debug, Clone)]
    struct MultiGndPart;
    impl Part for MultiGndPart {
        fn ref_prefix(&self) -> String {
            "U".to_string()
        }

        fn pin_ids(&self) -> Vec<String> {
            ["VCC", "GND1", "GND2", "GND3"].map(String::from).to_vec()
        }

        fn value(&self) -> String {
            "multi_gnd".to_string()
        }

        fn internal_connections(&self) -> Vec<Vec<String>> {
            vec![
                ["GND1", "GND2", "GND3", "unknown"]
                    .map(String::from)
                    .to_vec(),
            ]
        }
    }

    #[test]
    fn test_internal_connections_are_connected() {
        let mut circuit = CircuitBuilder::new();
        circuit.instantiate_part(MultiGndPart);
        let circuit = circuit.build(&());

        assert_eq!(
            connected_pin_names(&circuit),
            vec![vec!["U1.GND1", "U1.GND2", "U1.GND3"], vec!["U1.VCC"]]
        );
    }

    #[derive(Debug, Clone)]
    struct LabeledSubcircuitGenerator;
    impl SubcircuitGenerator for LabeledSubcircuitGenerator {
        type Options = ();
        type Interface = [Node; 1];

        // Interface node 0 is labeled "IN" inside; a part pin is labeled "OUT" (local)
        // and "VCC" (global).
        fn construct_subcircuit(
            &self,
            _options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            interface: Self::Interface,
        ) {
            let [input] = interface;
            circuit.label(input, NetLabel::local("IN"));
            let pins = TestPart::instantiate(circuit);
            circuit.connect(input, pins.pin1);
            circuit.label(pins.pin2, NetLabel::local("OUT"));
            let pins = TestPart::instantiate(circuit);
            circuit.label(pins.pin1, NetLabel::global("VCC"));
        }
    }

    #[test]
    fn test_labels_connect_nets() {
        let mut circuit = CircuitBuilder::new();
        // U1 is the top-level part; U2, U3 / U4, U5 are inside the subcircuits.
        let left = circuit.instantiate_subcircuit(Named("left", LabeledSubcircuitGenerator));
        circuit.instantiate_subcircuit(Named("right", LabeledSubcircuitGenerator));
        let u5 = TestPart::instantiate(&mut circuit);
        circuit.label(u5.pin1, NetLabel::global("VCC"));
        circuit.label(u5.pin2, NetLabel::local("OUT"));
        circuit.label(left[0], NetLabel::local("A"));
        let circuit = circuit.build(&());

        assert_eq!(
            connected_pin_names(&circuit),
            vec![
                vec!["U1.pin1", "U3.pin1", "U5.pin1"],
                vec!["U1.pin2"],
                vec!["U2.pin1"],
                vec!["U2.pin2"],
                vec!["U3.pin2"],
                vec!["U4.pin1"],
                vec!["U4.pin2"],
                vec!["U5.pin2"],
            ]
        );

        assert_eq!(
            paths(&circuit),
            vec!["/", "/left/", "/left/", "/right/", "/right/"]
        );

        // The outer node of the interface has both the outer and the inner label.
        let names: Vec<_> = circuit.nets[left[0].index()]
            .labels
            .iter()
            .map(NodeLabel::net_name)
            .collect();
        assert_eq!(names, vec!["/A", "/left/IN"]);
    }

    #[test]
    fn test_unused_names_still_connect() {
        let mut circuit = CircuitBuilder::new();
        let u1 = TestPart::instantiate(&mut circuit);
        let u2 = TestPart::instantiate(&mut circuit);
        circuit.label(u1.pin1, NetLabel::local("ALPHA"));
        circuit.label(u1.pin1, NetLabel::local("ZED"));
        circuit.label(u2.pin1, NetLabel::local("ZED"));
        let circuit = circuit.build(&());
        assert_eq!(
            connected_pin_names(&circuit),
            vec![vec!["U1.pin1", "U2.pin1"], vec!["U1.pin2"], vec!["U2.pin2"]]
        );
    }

    #[test]
    fn test_automatic_subcircuit_names() {
        let mut circuit = CircuitBuilder::new();
        TestSubcircuitGenerator2::instantiate(&mut circuit);
        TestSubcircuitGenerator1::instantiate(&mut circuit);
        TestSubcircuitGenerator2::instantiate(&mut circuit);
        let circuit = circuit.build(&());
        assert_eq!(
            paths(&circuit),
            vec![
                "/TestSubcircuitGenerator2#1/",
                "/TestSubcircuitGenerator2#1/TestSubcircuitGenerator1#1/",
                "/TestSubcircuitGenerator2#1/TestSubcircuitGenerator1#1/",
                "/TestSubcircuitGenerator1#1/",
                "/TestSubcircuitGenerator1#1/",
                "/TestSubcircuitGenerator2#2/",
                "/TestSubcircuitGenerator2#2/TestSubcircuitGenerator1#1/",
                "/TestSubcircuitGenerator2#2/TestSubcircuitGenerator1#1/",
            ]
        );
    }

    #[derive(Debug, Clone)]
    struct BadNamesGenerator;
    static BAD_NAME_LINES: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
    impl SubcircuitGenerator for BadNamesGenerator {
        type Options = ();
        type Interface = ();

        fn construct_subcircuit(
            &self,
            _options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            _interface: Self::Interface,
        ) {
            let first = line!() + 1;
            circuit.instantiate_subcircuit(Named("a", TestSubcircuitGenerator1));
            let second = line!() + 1;
            circuit.instantiate_subcircuit(Named("a", TestSubcircuitGenerator1));
            let hash = line!() + 1;
            circuit.instantiate_subcircuit(Named("b#1", TestSubcircuitGenerator1));
            let node = circuit.add_node();
            let label = line!() + 1;
            circuit.label(node, NetLabel::local(""));
            *BAD_NAME_LINES.lock().unwrap() = vec![first, second, hash, label];
        }
    }

    #[test]
    fn test_invalid_names_are_reported_with_traces() {
        let mut circuit = CircuitBuilder::new();
        let outer = line!() + 1;
        circuit.instantiate_subcircuit(BadNamesGenerator);
        let circuit = circuit.build(&());
        let [first, second, hash, label] = BAD_NAME_LINES.lock().unwrap()[..] else {
            panic!()
        };
        let errors = circuit.errors();
        assert_eq!(errors.len(), 3);
        assert_eq!(
            errors[0].kind,
            CircuitErrorKind::DuplicateInstanceName { name: "a".into() }
        );
        assert_eq!(lines(&errors[0].trace), vec![outer, second]);
        assert_eq!(lines(&errors[0].notes[0].trace), vec![outer, first]);
        assert!(
            matches!(&errors[1].kind, CircuitErrorKind::InvalidName { name, .. } if name == "b#1")
        );
        assert_eq!(lines(&errors[1].trace), vec![outer, hash]);
        assert!(
            matches!(&errors[2].kind, CircuitErrorKind::InvalidName { name, .. } if name.is_empty())
        );
        assert_eq!(lines(&errors[2].trace), vec![outer, label]);
    }

    #[test]
    fn test_escape_sequences_in_names_are_rejected() {
        // `I/O` and `I{slash}O` would otherwise become the same net, and the parts `a/b` and
        // `a{slash}b` would get the same UUID.
        let mut circuit = CircuitBuilder::<()>::new();
        let a = circuit.instantiate_part(Named("a/b", TestPart));
        let b = circuit.instantiate_part(Named("a{slash}b", TestPart));
        circuit.label(a["pin1"], NetLabel::global("I/O"));
        circuit.label(b["pin1"], NetLabel::global("I{slash}O"));
        let invalid = invalid_names(&circuit.build(&()));
        assert_eq!(invalid, ["a{slash}b", "I{slash}O"]);
    }

    #[test]
    fn test_invalid_ref_prefixes_and_base_names_are_rejected() {
        #[derive(Debug, Clone)]
        struct Based(&'static str);
        impl SubcircuitGenerator for Based {
            type Options = ();
            type Interface = ();
            fn construct_subcircuit(&self, _: &(), _: &mut CircuitBuilder<()>, _: ()) {}
            fn base_name(&self) -> String {
                self.0.into()
            }
        }
        let mut circuit = CircuitBuilder::<()>::new();
        for prefix in ["U1", "", "R 1", "U?", "#PWR", "a/b", "{x}", "Q"] {
            circuit.instantiate_part(crate::test_parts::CustomPart {
                ref_prefix: prefix,
                ..crate::test_parts::CustomPart::valid()
            });
        }
        circuit.instantiate_subcircuit(Based("{slash}"));
        circuit.instantiate_subcircuit(Based("a#1"));
        circuit.instantiate_subcircuit(Based(""));
        // Only used for automatic names.
        circuit.instantiate_subcircuit(Named("named", Based("{x}")));
        circuit.instantiate_subcircuit(Based("Stage2"));
        let invalid = invalid_names(&circuit.build(&()));
        assert_eq!(
            invalid,
            [
                "U1", "", "R 1", "U?", "#PWR", "a/b", "{x}", "{slash}", "a#1", ""
            ]
        );
    }

    #[derive(Debug, Clone)]
    struct NamedPartsGenerator {
        reversed: bool,
    }
    impl SubcircuitGenerator for NamedPartsGenerator {
        type Options = ();
        type Interface = ();

        fn construct_subcircuit(
            &self,
            _options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            _interface: Self::Interface,
        ) {
            let mut names = vec!["first", "second"];
            if self.reversed {
                names.reverse();
            }
            for name in names {
                circuit.instantiate_part(Named(name, TestPart));
            }
            circuit.instantiate_part(TestPart);
            circuit.instantiate_part(MultiGndPart);
        }
    }

    fn uuids_by_name(circuit: &BuiltCircuit) -> HashMap<String, uuid::Uuid> {
        circuit
            .parts
            .iter()
            .map(|part| (format!("{}{}", part.path, part.name), part.uuid()))
            .collect()
    }

    #[test]
    fn test_part_uuids_are_stable() {
        let build = |reversed: bool, extra_part: bool| {
            let mut circuit = CircuitBuilder::new();
            if extra_part {
                circuit.instantiate_part(MultiGndPart);
            }
            circuit.instantiate_subcircuit(Named("sub", NamedPartsGenerator { reversed }));
            circuit.build(&())
        };

        let base = build(false, false);
        let reordered = build(true, false);
        let names: Vec<_> = uuids_by_name(&base).into_keys().sorted().collect();
        assert_eq!(
            names,
            vec!["/sub/U#1", "/sub/U#2", "/sub/first", "/sub/second"]
        );
        // Reordering explicitly named parts keeps every UUID even though the references change.
        assert_eq!(uuids_by_name(&base), uuids_by_name(&reordered));
        let ref_of = |circuit: &BuiltCircuit, name: &str| {
            let index = circuit.parts.iter().position(|part| part.name == name);
            circuit.ref_names[index.unwrap()].clone()
        };
        assert_eq!(ref_of(&base, "first"), "U1");
        assert_eq!(ref_of(&reordered, "first"), "U2");

        // Adding a part outside the subcircuit does not change the UUIDs inside it.
        let extended = build(false, true);
        let mut extended_uuids = uuids_by_name(&extended);
        extended_uuids.remove("/U#1");
        assert_eq!(uuids_by_name(&base), extended_uuids);

        let all: HashSet<_> = uuids_by_name(&extended).into_values().collect();
        assert_eq!(all.len(), extended.parts.len());
    }

    // A part between the interface nodes 0 (`pin1`) and 1 (`pin2`).
    #[derive(Debug, Clone)]
    struct InterfaceGenerator;
    impl SubcircuitGenerator for InterfaceGenerator {
        type Options = ();
        type Interface = [Node; 2];

        fn construct_subcircuit(
            &self,
            _options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            interface: Self::Interface,
        ) {
            let [first, second] = interface;
            let part = TestPart::instantiate(circuit);
            circuit.connect(first, part.pin1);
            circuit.connect(second, part.pin2);
        }
    }

    #[test]
    fn test_foreign_nodes_are_reported() {
        /// Uses a node of the circuit around it.
        #[derive(Debug, Clone)]
        struct UsesOuterNode(Node);
        impl SubcircuitGenerator for UsesOuterNode {
            type Options = ();
            type Interface = [Node; 1];
            fn construct_subcircuit(
                &self,
                _: &(),
                circuit: &mut CircuitBuilder,
                [inner]: [Node; 1],
            ) {
                circuit.connect(inner, self.0);
                circuit.label(self.0, NetLabel::local("X"));
            }
        }

        let mut circuit = CircuitBuilder::new();
        let outer = circuit.add_node();
        circuit.instantiate_subcircuit(UsesOuterNode(outer));
        // A clone shares the nodes.
        let mut clone = circuit.clone();
        clone.no_connect(outer);
        assert!(clone.errors.is_empty());
        let other = CircuitBuilder::<()>::new().add_node();
        let other_line = line!() + 1;
        circuit.no_connect(other);

        let built = circuit.build(&());
        assert!(
            built
                .errors()
                .iter()
                .all(|error| matches!(error.kind, CircuitErrorKind::ForeignNode))
        );
        assert_eq!(built.errors().len(), 3);
        assert_eq!(lines(&built.errors()[0].trace), vec![other_line]);
        // The uses are ignored: the inner node is not connected to the outer one, nor labeled.
        assert!(built.nets.node_weights().all(|node| node.labels.is_empty()));
        assert_eq!(built.nets.edge_count(), 0);
    }

    /// The pin connected to `node` through the net it belongs to.
    fn pin_on(circuit: &BuiltCircuit, node: Node) -> String {
        let pins = pins_by_node(circuit);
        let net = circuit
            .connected_nodes()
            .into_iter()
            .find(|net| net.contains(&node.index()))
            .unwrap();
        let mut ids: Vec<_> = net
            .iter()
            .filter_map(|node| pins.get(node))
            .map(|(_, pin_id)| pin_id.clone())
            .collect();
        assert_eq!(ids.len(), 1, "{ids:?}");
        ids.pop().unwrap()
    }

    #[test]
    fn test_instantiate_subcircuit_keeps_the_order_of_an_array() {
        let mut circuit = CircuitBuilder::new();
        let [first, second] = circuit.instantiate_subcircuit(InterfaceGenerator);
        let [named_first, named_second] =
            circuit.instantiate_subcircuit(Named("b", InterfaceGenerator));
        let names: Vec<_> = circuit
            .subcircuits
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["InterfaceGenerator#1", "b"]);
        let circuit = circuit.build(&());
        assert_eq!(pin_on(&circuit, first), "pin1");
        assert_eq!(pin_on(&circuit, second), "pin2");
        assert_eq!(pin_on(&circuit, named_first), "pin1");
        assert_eq!(pin_on(&circuit, named_second), "pin2");
    }

    #[derive(crate::subcircuit::Interface)]
    struct Crossed<T = Node> {
        to_pin2: T,
        to_pin1: T,
    }

    // A part with its pins connected to the fields named after them.
    #[derive(Debug, Clone)]
    struct CrossedGenerator;
    impl SubcircuitGenerator for CrossedGenerator {
        type Options = ();
        type Interface = Crossed;

        fn construct_subcircuit(
            &self,
            _options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            interface: Self::Interface,
        ) {
            let part = TestPart::instantiate(circuit);
            circuit.connect(interface.to_pin1, part.pin1);
            circuit.connect(interface.to_pin2, part.pin2);
        }
    }

    #[test]
    fn test_instantiate_subcircuit_with_a_derived_interface() {
        let mut circuit = CircuitBuilder::new();
        let ports = circuit.instantiate_subcircuit(CrossedGenerator);
        let circuit = circuit.build(&());
        assert_eq!(pin_on(&circuit, ports.to_pin1), "pin1");
        assert_eq!(pin_on(&circuit, ports.to_pin2), "pin2");
    }
}
