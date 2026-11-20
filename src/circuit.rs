use std::{collections::HashMap, fmt::Debug, panic::Location};

use petgraph::{
    graph::{NodeIndex, UnGraph},
    unionfind::UnionFind,
};

use crate::{
    erc::{ErcMark, NodeMark},
    error::{CircuitError, CircuitErrorKind},
    naming::{AUTO_NAME_SEPARATOR, HierPath, NetLabel, NodeLabel, check_explicit_name},
    parts::{Part, PartInstance, PinRef, PinType},
    subcircuit::{Interface, SubcircuitGenerator, SubcircuitInstance},
    trace::{Loc, Trace},
};

/// Weight of a node in the net graph.
#[derive(Debug, Clone)]
pub struct NodeInfo {
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

    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn labels(&self) -> &[NodeLabel] {
        &self.labels
    }

    pub fn marks(&self) -> &[NodeMark] {
        &self.marks
    }
}

/// Nodes carry their creation location and labels, and edges carry the location where they were connected.
pub type NetGraph = UnGraph<NodeInfo, Loc>;

/// Assigns instance names in one (sub)circuit: explicit names are checked for validity and
/// uniqueness, and automatic names are numbered per base name (`R#1`, `R#2`, ...).
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

pub struct CircuitBuilder<B = ()> {
    pub(crate) parts: Vec<PartInstance>,
    pub(crate) ref_name_counters: HashMap<String, u32>,
    pub(crate) subcircuits: Vec<SubcircuitInstance<B>>,
    pub(crate) nets: NetGraph,
    pub(crate) subcircuit_names: NameRegistry,
    pub(crate) part_names: NameRegistry,
    pub(crate) errors: Vec<CircuitError>,
}

impl<B> Clone for CircuitBuilder<B> {
    fn clone(&self) -> Self {
        Self {
            parts: self.parts.clone(),
            ref_name_counters: self.ref_name_counters.clone(),
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
            .field("ref_name_counters", &self.ref_name_counters)
            .field("subcircuits", &self.subcircuits)
            .field("nets", &self.nets)
            .field("errors", &self.errors)
            .finish_non_exhaustive()
    }
}

impl<B> CircuitBuilder<B> {
    pub fn new() -> Self {
        CircuitBuilder {
            parts: Vec::new(),
            subcircuits: Vec::new(),
            nets: NetGraph::new_undirected(),
            ref_name_counters: HashMap::new(),
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
            parts,
            ref_name_counters,
            subcircuits,
            nets,
            subcircuit_names: _,
            part_names: _,
            errors,
        } = self;
        let mut expanded = ExpandedCircuit {
            parts,
            ref_name_counters,
            nets,
            errors,
        };

        for instance in subcircuits {
            let subcircuit = instance.generator.generate(options);
            expanded.merge(subcircuit, &instance);
        }

        expanded
    }

    /// Creates a new node (a point that pins and other nodes can be connected to).
    #[track_caller]
    pub fn add_node(&mut self) -> NodeIndex {
        self.add_node_at(Location::caller())
    }

    pub(crate) fn add_node_at(&mut self, location: &'static Location<'static>) -> NodeIndex {
        self.nets.add_node(NodeInfo::new(Trace::new(location)))
    }

    /// Connects two nodes.
    #[track_caller]
    pub fn connect(&mut self, a: NodeIndex, b: NodeIndex) {
        self.nets.add_edge(a, b, Location::caller());
    }

    /// Gives a name to the net containing `node`.
    ///
    /// Nodes with the same global label, or with the same local label in the same
    /// (sub)circuit instance, are connected. If a net ends up with several names,
    /// one of them is used (see the netlist conversion) and a warning is reported.
    #[track_caller]
    pub fn label(&mut self, node: NodeIndex, label: NetLabel) {
        let trace = Trace::new(Location::caller());
        if label.name().is_empty() {
            self.errors.push(CircuitError::new(
                CircuitErrorKind::InvalidName {
                    name: String::new(),
                    reason: "net labels must not be empty",
                },
                trace.clone(),
            ));
        }
        self.nets[node].labels.push(NodeLabel {
            label,
            scope: HierPath::default(),
            trace,
        });
    }

    /// Marks the net containing `node` as intentionally unconnected (KiCad's no-connect flag).
    /// ERC reports the mark if the net has more than one pin, or no pin.
    #[track_caller]
    pub fn no_connect(&mut self, node: NodeIndex) {
        self.mark(node, ErcMark::NoConnect);
    }

    /// Marks the net containing `node` as driven by a power source outside the circuit
    /// (KiCad's `PWR_FLAG`): a virtual `power_out` pin named `PWR_FLAG`.
    #[track_caller]
    pub fn power_flag(&mut self, node: NodeIndex) {
        self.virtual_pin(node, PinType::PowerOut, "PWR_FLAG");
    }

    /// Adds a pin outside the circuit to the net containing `node`, seen only by ERC
    /// (e.g. what an off-board connection or the circuit around a subcircuit provides).
    #[track_caller]
    pub fn virtual_pin(&mut self, node: NodeIndex, pin_type: PinType, name: &str) {
        self.mark(
            node,
            ErcMark::VirtualPin {
                pin_type,
                name: name.to_string(),
            },
        );
    }

    #[track_caller]
    fn mark(&mut self, node: NodeIndex, mark: ErcMark) {
        let trace = Trace::new(Location::caller());
        self.nets[node].marks.push(NodeMark { mark, trace });
    }

    /// Instantiates a subcircuit with an automatic name (`<base name>#<n>`) and returns its
    /// interface: new nodes in this circuit, connected to the subcircuit's interface nodes.
    #[track_caller]
    pub fn instantiate_subcircuit<G, F>(
        &mut self,
        generator: G,
        options_for_generator: F,
    ) -> G::Interface
    where
        B: 'static,
        G: SubcircuitGenerator + 'static,
        F: Fn(&B) -> G::Options + Clone + 'static,
    {
        self.instantiate_subcircuit_impl(None, generator, options_for_generator)
    }

    /// Instantiates a subcircuit with an explicit name, used in hierarchical net names
    /// (`/name/LABEL`) and as the key of the deterministic UUIDs of the parts inside it.
    /// Returns its interface like [`Self::instantiate_subcircuit`].
    #[track_caller]
    pub fn instantiate_subcircuit_named<G, F>(
        &mut self,
        name: &str,
        generator: G,
        options_for_generator: F,
    ) -> G::Interface
    where
        B: 'static,
        G: SubcircuitGenerator + 'static,
        F: Fn(&B) -> G::Options + Clone + 'static,
    {
        self.instantiate_subcircuit_impl(Some(name), generator, options_for_generator)
    }

    #[track_caller]
    fn instantiate_subcircuit_impl<G, F>(
        &mut self,
        name: Option<&str>,
        generator: G,
        options_for_generator: F,
    ) -> G::Interface
    where
        B: 'static,
        G: SubcircuitGenerator + 'static,
        F: Fn(&B) -> G::Options + Clone + 'static,
    {
        let location = Location::caller();
        let name = self.subcircuit_names.assign(
            name,
            &generator.base_name(),
            &Trace::new(location),
            &mut self.errors,
        );
        // The subcircuit creates its interface nodes first (`subcircuit::construct`).
        let interface = G::Interface::from_fn(|_| self.add_node_at(location));
        let node_mapping = interface
            .values()
            .into_iter()
            .enumerate()
            .map(|(index, &node)| (NodeIndex::new(index), node))
            .collect();
        self.subcircuits.push(SubcircuitInstance::new(
            generator,
            options_for_generator,
            node_mapping,
            name,
            location,
        ));
        interface
    }

    /// Instantiates a part with an automatic name (`<ref base name>#<n>`) and returns
    /// the nodes of its pins, keyed by pin ID.
    ///
    /// The instance name is the key of the deterministic UUID of the part. Automatic names
    /// shift when a part of the same kind is added or removed before this one in the same
    /// (sub)circuit; use [`Self::instantiate_part_named`] to keep the UUID stable.
    #[track_caller]
    pub fn instantiate_part(&mut self, part: Box<dyn Part>) -> HashMap<String, NodeIndex> {
        self.instantiate_part_impl(None, part)
    }

    /// Instantiates a part with an explicit name, unique in this (sub)circuit.
    #[track_caller]
    pub fn instantiate_part_named(
        &mut self,
        name: &str,
        part: Box<dyn Part>,
    ) -> HashMap<String, NodeIndex> {
        self.instantiate_part_impl(Some(name), part)
    }

    #[track_caller]
    fn instantiate_part_impl(
        &mut self,
        name: Option<&str>,
        part: Box<dyn Part>,
    ) -> HashMap<String, NodeIndex> {
        let trace = Trace::new(Location::caller());
        let name =
            self.part_names
                .assign(name, &part.get_ref_base_name(), &trace, &mut self.errors);
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
        let ref_number = next_ref_number(&mut self.ref_name_counters, part.as_ref());
        self.parts.push(PartInstance {
            part,
            ref_number,
            pin_mapping: pin_mapping.clone(),
            name,
            path: HierPath::default(),
            trace,
        });
        pin_mapping
    }
}

impl<B> Default for CircuitBuilder<B> {
    fn default() -> Self {
        Self::new()
    }
}

fn next_ref_number(counters: &mut HashMap<String, u32>, part: &dyn Part) -> u32 {
    let counter = counters.entry(part.get_ref_base_name()).or_insert(1);
    let ref_number = *counter;
    *counter += 1;
    ref_number
}

#[derive(Debug, Clone)]
pub struct ExpandedCircuit {
    parts: Vec<PartInstance>,
    ref_name_counters: HashMap<String, u32>,
    nets: NetGraph,
    errors: Vec<CircuitError>,
}

impl ExpandedCircuit {
    /// Merges an expanded subcircuit generated for `instance`.
    fn merge<B>(&mut self, other: ExpandedCircuit, instance: &SubcircuitInstance<B>) {
        let location = instance.location;
        let name = instance.name.as_str();
        let outer_label = |label: &NodeLabel| NodeLabel {
            label: label.label.clone(),
            scope: label.scope.prepended(name),
            trace: label.trace.prepended(location),
        };

        let mut index_map = HashMap::<NodeIndex, NodeIndex>::new();
        for node in other.nets.node_indices() {
            let inner = &other.nets[node];
            let labels = inner.labels.iter().map(outer_label);
            let marks = inner.marks.iter().map(|mark| NodeMark {
                mark: mark.mark.clone(),
                trace: mark.trace.prepended(location),
            });
            let merged_node = match instance.node_mapping.get(&node) {
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
            let ref_number = next_ref_number(&mut self.ref_name_counters, part.part.as_ref());
            self.parts.push(PartInstance {
                ref_number,
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
    fn from(expanded: ExpandedCircuit) -> Self {
        BuiltCircuit {
            parts: expanded.parts,
            nets: expanded.nets,
            errors: expanded.errors,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuiltCircuit {
    pub(crate) parts: Vec<PartInstance>,
    pub(crate) nets: NetGraph,
    /// Errors found while describing the circuit (reported by the netlist conversion).
    pub(crate) errors: Vec<CircuitError>,
}

impl BuiltCircuit {
    pub fn nets(&self) -> &NetGraph {
        &self.nets
    }

    pub fn errors(&self) -> &[CircuitError] {
        &self.errors
    }

    pub fn pin_refs_by_node(&self) -> HashMap<NodeIndex, PinRef> {
        self.parts
            .iter()
            .flat_map(|part| {
                part.pin_mapping.iter().map(|(pin_id, &node_index)| {
                    (
                        node_index,
                        PinRef {
                            node_index,
                            part_ref_name: part.ref_name(),
                            pin_id: pin_id.clone(),
                        },
                    )
                })
            })
            .collect()
    }

    /// Groups nodes into nets: nodes connected by edges, by the same global label,
    /// or by the same local label in the same subcircuit instance.
    /// Each group is sorted, and groups are sorted by their first node.
    pub fn connected_nodes(&self) -> Vec<Vec<NodeIndex>> {
        let mut union_find = UnionFind::<usize>::new(self.nets.node_count());
        for edge in self.nets.edge_indices() {
            let (a, b) = self.nets.edge_endpoints(edge).unwrap();
            union_find.union(a.index(), b.index());
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

    pub fn collect_connected_part_pins(&self) -> Vec<Vec<PinRef>> {
        let pin_refs = self.pin_refs_by_node();
        self.connected_nodes()
            .into_iter()
            .map(|nodes| {
                nodes
                    .into_iter()
                    .filter_map(|node| pin_refs.get(&node).cloned())
                    .collect()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;
    use petgraph::graph::NodeIndex;
    use std::collections::{HashMap, HashSet};

    use crate::{
        parts::{Part, PinRef},
        subcircuit::SubcircuitGenerator,
    };

    use super::*;

    #[derive(Debug, Clone)]
    struct TestPart;
    struct TestPartPins {
        pub pin1: NodeIndex,
        pub pin2: NodeIndex,
    }
    impl Part for TestPart {
        fn get_ref_base_name(&self) -> String {
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
            let pins = circuit.instantiate_part(Box::new(TestPart));
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
        pub pin1: NodeIndex,
        pub pin2: NodeIndex,
    }
    impl SubcircuitGenerator for TestSubcircuitGenerator1 {
        type Options = ();
        type Interface = [NodeIndex; 2];

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
            let [pin1, pin2] =
                circuit.instantiate_subcircuit(TestSubcircuitGenerator1, |options: &()| *options);
            TestSubcircuitGenerator1Pins { pin1, pin2 }
        }
    }

    // Subcircuit containing a part and an instance of the first subcircuit connected in series
    #[derive(Debug, Clone)]
    struct TestSubcircuitGenerator2;
    #[allow(dead_code)]
    struct TestSubcircuitGenerator2Pins {
        pub pin1: NodeIndex,
        pub pin2: NodeIndex,
    }
    impl SubcircuitGenerator for TestSubcircuitGenerator2 {
        type Options = ();
        type Interface = [NodeIndex; 2];

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
            let [pin1, pin2] =
                circuit.instantiate_subcircuit(TestSubcircuitGenerator2, |options: &()| *options);
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
        type Interface = [NodeIndex; 2];

        fn construct_subcircuit(
            &self,
            _options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            interface: Self::Interface,
        ) {
            let [first, second] = interface;
            let [inner_first, inner_second] =
                circuit.instantiate_subcircuit(LeafOptionsGenerator, |options: &NestedOptions| {
                    LeafOptions {
                        part_count: options.part_count,
                    }
                });
            circuit.connect(first, inner_first);
            circuit.connect(second, inner_second);
        }
    }

    #[derive(Debug, Clone)]
    struct LeafOptionsGenerator;

    impl SubcircuitGenerator for LeafOptionsGenerator {
        type Options = LeafOptions;
        type Interface = [NodeIndex; 2];

        fn construct_subcircuit(
            &self,
            options: &Self::Options,
            circuit: &mut CircuitBuilder<Self::Options>,
            _interface: Self::Interface,
        ) {
            for _ in 0..options.part_count {
                circuit.instantiate_part(Box::new(TestPart));
            }
        }
    }

    #[test]
    fn test_generator_specific_options_expand_recursively() {
        let mut circuit = CircuitBuilder::<BuildOptions>::new();
        circuit.instantiate_subcircuit(NestedOptionsGenerator, |options: &BuildOptions| {
            NestedOptions {
                part_count: options.part_count,
            }
        });

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
        // number of nodes before expansion should be 2 (the interface nodes of the top-level subcircuit)
        assert_eq!(circuit.nets.node_count(), 2);
        let circuit = circuit.build(&());
        // we should have 3 part instance (the 1 inside the top-level subcircuit, and the 2 inside the subcircuit instance inside it)
        assert_eq!(circuit.parts.len(), 3);
        // we should have 2 + 4 + 4 = 10 nodes: 2 from the top-level subcircuit, 4 from the subcircuit instance inside it, and 4 from the part instance inside that
        assert_eq!(circuit.nets.node_count(), 2 + 4 + 4);
        // we should have 3 + 4 = 7 edges: 3 from the top-level subcircuit, and 4 from the subcircuit instance inside it
        assert_eq!(circuit.nets.edge_count(), 3 + 4);
    }

    #[test]
    fn test_pin_refs_by_node() {
        let mut circuit = CircuitBuilder::new();
        let _subcircuit_pins2 = TestSubcircuitGenerator2::instantiate(&mut circuit);
        let circuit = circuit.build(&());

        let expected = HashMap::from([
            (
                NodeIndex::new(4),
                PinRef {
                    node_index: NodeIndex::new(4),
                    part_ref_name: "U1".to_string(),
                    pin_id: "pin1".to_string(),
                },
            ),
            (
                NodeIndex::new(5),
                PinRef {
                    node_index: NodeIndex::new(5),
                    part_ref_name: "U1".to_string(),
                    pin_id: "pin2".to_string(),
                },
            ),
            (
                NodeIndex::new(6),
                PinRef {
                    node_index: NodeIndex::new(6),
                    part_ref_name: "U2".to_string(),
                    pin_id: "pin1".to_string(),
                },
            ),
            (
                NodeIndex::new(7),
                PinRef {
                    node_index: NodeIndex::new(7),
                    part_ref_name: "U2".to_string(),
                    pin_id: "pin2".to_string(),
                },
            ),
            (
                NodeIndex::new(8),
                PinRef {
                    node_index: NodeIndex::new(8),
                    part_ref_name: "U3".to_string(),
                    pin_id: "pin1".to_string(),
                },
            ),
            (
                NodeIndex::new(9),
                PinRef {
                    node_index: NodeIndex::new(9),
                    part_ref_name: "U3".to_string(),
                    pin_id: "pin2".to_string(),
                },
            ),
        ]);

        assert_eq!(circuit.pin_refs_by_node(), expected);
    }

    #[test]
    fn test_merge_reassigns_ref_numbers() {
        let mut circuit = CircuitBuilder::new();
        TestPart::instantiate(&mut circuit);
        let _subcircuit_pins = TestSubcircuitGenerator2::instantiate(&mut circuit);

        let circuit = circuit.build(&());

        let mut ref_names: Vec<_> = circuit
            .pin_refs_by_node()
            .values()
            .map(|pin_ref| pin_ref.part_ref_name.clone())
            .collect();
        ref_names.sort();
        ref_names.dedup();
        assert_eq!(ref_names, vec!["U1", "U2", "U3", "U4"]);
    }

    #[test]
    fn test_collect_connected_part_pins() {
        let mut circuit = CircuitBuilder::new();
        let _subcircuit_pins2 = TestSubcircuitGenerator2::instantiate(&mut circuit);
        let circuit = circuit.build(&());

        let actual: Vec<Vec<PinRef>> = circuit
            .collect_connected_part_pins()
            .into_iter()
            .map(|inner| inner.into_iter().sorted().collect())
            .sorted()
            .collect();

        let expected: Vec<Vec<PinRef>> = (vec![
            vec![
                PinRef {
                    node_index: NodeIndex::new(7),
                    part_ref_name: "U2".to_string(),
                    pin_id: "pin2".to_string(),
                },
                PinRef {
                    node_index: NodeIndex::new(9),
                    part_ref_name: "U3".to_string(),
                    pin_id: "pin2".to_string(),
                },
                PinRef {
                    node_index: NodeIndex::new(4),
                    part_ref_name: "U1".to_string(),
                    pin_id: "pin1".to_string(),
                },
            ],
            vec![PinRef {
                node_index: NodeIndex::new(5),
                part_ref_name: "U1".to_string(),
                pin_id: "pin2".to_string(),
            }],
            vec![
                PinRef {
                    node_index: NodeIndex::new(6),
                    part_ref_name: "U2".to_string(),
                    pin_id: "pin1".to_string(),
                },
                PinRef {
                    node_index: NodeIndex::new(8),
                    part_ref_name: "U3".to_string(),
                    pin_id: "pin1".to_string(),
                },
            ],
        ])
        .into_iter()
        .map(|inner| inner.into_iter().sorted().collect())
        .sorted()
        .collect();

        assert_eq!(actual, expected);
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
            circuit.instantiate_part(Box::new(TestPart));
            *TRACED_PART_LINE.lock().unwrap() = line;
        }
    }

    #[test]
    fn test_traces_follow_subcircuit_instantiations() {
        let mut circuit = CircuitBuilder::new();
        let part_line = line!() + 1;
        let _ = TestPart::instantiate(&mut circuit);
        let subcircuit_line = line!() + 1;
        circuit.instantiate_subcircuit(TracedSubcircuitGenerator, |o: &()| *o);
        let node_line = line!() + 1;
        let node = circuit.add_node();
        let circuit = circuit.build(&());

        let lines = |trace: &Trace| trace.0.iter().map(|loc| loc.line()).collect::<Vec<_>>();
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
        assert_eq!(lines(&circuit.nets[node].trace), vec![node_line]);
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
        fn get_ref_base_name(&self) -> String {
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
        circuit.instantiate_part(Box::new(MultiGndPart));
        let circuit = circuit.build(&());

        let groups: Vec<Vec<String>> = circuit
            .collect_connected_part_pins()
            .into_iter()
            .map(|pins| pins.into_iter().map(|pin| pin.pin_id).sorted().collect())
            .sorted()
            .collect();
        assert_eq!(groups, vec![vec!["GND1", "GND2", "GND3"], vec!["VCC"]]);
    }

    #[derive(Debug, Clone)]
    struct LabeledSubcircuitGenerator;
    impl SubcircuitGenerator for LabeledSubcircuitGenerator {
        type Options = ();
        type Interface = [NodeIndex; 1];

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

    fn connected_pin_names(circuit: &BuiltCircuit) -> Vec<Vec<String>> {
        circuit
            .collect_connected_part_pins()
            .into_iter()
            .map(|pins| {
                pins.into_iter()
                    .map(|pin| format!("{}.{}", pin.part_ref_name, pin.pin_id))
                    .sorted()
                    .collect::<Vec<_>>()
            })
            .filter(|pins| !pins.is_empty())
            .sorted()
            .collect()
    }

    #[test]
    fn test_labels_connect_nets() {
        let mut circuit = CircuitBuilder::new();
        // U1 is the top-level part; U2, U3 / U4, U5 are inside the subcircuits.
        let left =
            circuit.instantiate_subcircuit_named("left", LabeledSubcircuitGenerator, |o: &()| *o);
        circuit.instantiate_subcircuit_named("right", LabeledSubcircuitGenerator, |o: &()| *o);
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

        let paths: Vec<_> = circuit
            .parts
            .iter()
            .map(|part| part.path.to_string())
            .collect();
        assert_eq!(paths, vec!["/", "/left/", "/left/", "/right/", "/right/"]);

        // The outer node of the interface has both the outer and the inner label.
        let names: Vec<_> = circuit.nets[left[0]]
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
        let paths: Vec<_> = circuit
            .parts
            .iter()
            .map(|part| part.path.to_string())
            .collect();
        assert_eq!(
            paths,
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
            circuit.instantiate_subcircuit_named("a", TestSubcircuitGenerator1, |o: &()| *o);
            let second = line!() + 1;
            circuit.instantiate_subcircuit_named("a", TestSubcircuitGenerator1, |o: &()| *o);
            let hash = line!() + 1;
            circuit.instantiate_subcircuit_named("b#1", TestSubcircuitGenerator1, |o: &()| *o);
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
        circuit.instantiate_subcircuit(BadNamesGenerator, |o: &()| *o);
        let circuit = circuit.build(&());
        let [first, second, hash, label] = BAD_NAME_LINES.lock().unwrap()[..] else {
            panic!()
        };
        let lines = |trace: &Trace| trace.0.iter().map(|loc| loc.line()).collect::<Vec<_>>();

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
                circuit.instantiate_part_named(name, Box::new(TestPart));
            }
            circuit.instantiate_part(Box::new(TestPart));
            circuit.instantiate_part(Box::new(MultiGndPart));
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
                circuit.instantiate_part(Box::new(MultiGndPart));
            }
            circuit.instantiate_subcircuit_named(
                "sub",
                NamedPartsGenerator { reversed },
                |o: &()| *o,
            );
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
            circuit
                .parts
                .iter()
                .find(|part| part.name == name)
                .unwrap()
                .ref_name()
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
        type Interface = [NodeIndex; 2];

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

    /// The pin connected to `node` through the net it belongs to.
    fn pin_on(circuit: &BuiltCircuit, node: NodeIndex) -> String {
        let pins = circuit.pin_refs_by_node();
        let net = circuit
            .connected_nodes()
            .into_iter()
            .find(|net| net.contains(&node))
            .unwrap();
        let mut ids: Vec<_> = net
            .iter()
            .filter_map(|node| pins.get(node))
            .map(|pin| pin.pin_id.clone())
            .collect();
        assert_eq!(ids.len(), 1, "{ids:?}");
        ids.pop().unwrap()
    }

    #[test]
    fn test_instantiate_subcircuit_keeps_the_order_of_an_array() {
        let mut circuit = CircuitBuilder::new();
        let [first, second] = circuit.instantiate_subcircuit(InterfaceGenerator, |o: &()| *o);
        let [named_first, named_second] =
            circuit.instantiate_subcircuit_named("b", InterfaceGenerator, |o: &()| *o);
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
    struct Crossed<T = NodeIndex> {
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
        let ports = circuit.instantiate_subcircuit(CrossedGenerator, |o: &()| *o);
        let circuit = circuit.build(&());
        assert_eq!(pin_on(&circuit, ports.to_pin1), "pin1");
        assert_eq!(pin_on(&circuit, ports.to_pin2), "pin2");
    }
}
