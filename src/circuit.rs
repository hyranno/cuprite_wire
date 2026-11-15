use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
};

use petgraph::{
    algo::kosaraju_scc,
    graph::{NodeIndex, UnGraph},
};

use crate::{
    parts::{Part, PartInstance, PinRef},
    subcircuit::{SubcircuitGenerator, SubcircuitInstance},
    trace::{Loc, Trace},
};

/// Weight of a node in the net graph.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// Where the node was created.
    pub(crate) trace: Trace,
}

impl NodeInfo {
    fn new(trace: Trace) -> Self {
        NodeInfo { trace }
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }
}

/// Nodes carry their creation location and edges carry the location where they were connected.
pub type NetGraph = UnGraph<NodeInfo, Loc>;

pub struct CircuitBuilder<B = ()> {
    pub(crate) parts: Vec<PartInstance>,
    pub(crate) ref_name_counters: HashMap<String, u32>,
    pub(crate) subcircuits: Vec<SubcircuitInstance<B>>,
    pub(crate) nets: NetGraph,
}

impl<B> Clone for CircuitBuilder<B> {
    fn clone(&self) -> Self {
        Self {
            parts: self.parts.clone(),
            ref_name_counters: self.ref_name_counters.clone(),
            subcircuits: self.subcircuits.clone(),
            nets: self.nets.clone(),
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
            .finish()
    }
}

impl<B> CircuitBuilder<B> {
    pub fn new() -> Self {
        CircuitBuilder {
            parts: Vec::new(),
            subcircuits: Vec::new(),
            nets: NetGraph::new_undirected(),
            ref_name_counters: HashMap::new(),
        }
    }

    pub fn build(&self, options: &B) -> BuiltCircuit {
        self.clone().expand(options).into()
    }

    pub(crate) fn expand(self, options: &B) -> ExpandedCircuit {
        let CircuitBuilder {
            parts,
            ref_name_counters,
            subcircuits,
            nets,
        } = self;
        let mut expanded = ExpandedCircuit {
            parts,
            ref_name_counters,
            nets,
        };

        for instance in subcircuits {
            let subcircuit = instance.generator.generate(options);
            expanded.merge(subcircuit, &instance.node_mapping, instance.location);
        }

        expanded
    }

    /// Creates a new node (a point that pins and other nodes can be connected to).
    #[track_caller]
    pub fn add_node(&mut self) -> NodeIndex {
        self.nets.add_node(NodeInfo::new(Trace::new(std::panic::Location::caller())))
    }

    /// Connects two nodes.
    #[track_caller]
    pub fn connect(&mut self, a: NodeIndex, b: NodeIndex) {
        self.nets.add_edge(a, b, std::panic::Location::caller());
    }

    /// Instantiates a subcircuit. `interface_nodes` are node indices inside the generated
    /// subcircuit; the returned map gives the corresponding new nodes in this circuit.
    #[track_caller]
    pub fn instantiate_subcircuit<G, F>(
        &mut self,
        generator: G,
        interface_nodes: &HashSet<NodeIndex>,
        options_for_generator: F,
    ) -> HashMap<NodeIndex, NodeIndex>
    where
        B: 'static,
        G: SubcircuitGenerator + 'static,
        F: Fn(&B) -> G::Options + Clone + 'static,
    {
        let location = std::panic::Location::caller();
        let mut sorted_interface_nodes: Vec<_> = interface_nodes.iter().copied().collect();
        sorted_interface_nodes.sort();
        let node_mapping: HashMap<_, _> = sorted_interface_nodes
            .into_iter()
            .map(|node| (node, self.nets.add_node(NodeInfo::new(Trace::new(location)))))
            .collect();
        self.subcircuits.push(SubcircuitInstance::new(
            generator,
            options_for_generator,
            node_mapping.clone(),
            location,
        ));
        node_mapping
    }

    /// Instantiates a part and returns the nodes of its pins, keyed by pin ID.
    #[track_caller]
    pub fn instantiate_part(&mut self, part: Box<dyn Part>) -> HashMap<String, NodeIndex> {
        let trace = Trace::new(std::panic::Location::caller());
        let mut pin_mapping = HashMap::new();
        for pin in part.pin_ids() {
            pin_mapping.insert(pin, self.nets.add_node(NodeInfo::new(trace.clone())));
        }
        // Unknown pin IDs are skipped here and reported when the netlist is created.
        for group in part.internal_connections() {
            let nodes: Vec<_> = group.iter().filter_map(|pin| pin_mapping.get(pin)).collect();
            for pair in nodes.windows(2) {
                self.nets.add_edge(*pair[0], *pair[1], trace.0[0]);
            }
        }
        let ref_base_name = part.get_ref_base_name();
        let counter = self.ref_name_counters.entry(ref_base_name).or_insert(1);
        let ref_number = *counter;
        *counter += 1;
        self.parts.push(PartInstance {
            part,
            ref_number,
            pin_mapping: pin_mapping.clone(),
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

#[derive(Debug, Clone)]
pub struct ExpandedCircuit {
    parts: Vec<PartInstance>,
    ref_name_counters: HashMap<String, u32>,
    nets: NetGraph,
}

impl ExpandedCircuit {
    /// Merges an expanded subcircuit that was instantiated at `location`.
    /// `overriding_nodes` maps the subcircuit's interface nodes to nodes in this circuit.
    fn merge(
        &mut self,
        other: ExpandedCircuit,
        overriding_nodes: &HashMap<NodeIndex, NodeIndex>,
        location: Loc,
    ) {
        let mut index_map = HashMap::<NodeIndex, NodeIndex>::new();
        for node in other.nets.node_indices() {
            let merged_node = match overriding_nodes.get(&node) {
                // The interface node keeps the information of the outer node.
                Some(&merged_node) => merged_node,
                None => self
                    .nets
                    .add_node(NodeInfo::new(other.nets[node].trace.prepended(location))),
            };
            index_map.insert(node, merged_node);
        }

        for edge in other.nets.edge_indices() {
            let (source, target) = other.nets.edge_endpoints(edge).unwrap();
            self.nets.add_edge(
                index_map[&source],
                index_map[&target],
                other.nets[edge],
            );
        }

        for part in other.parts {
            let new_pin_mapping = part
                .pin_mapping
                .iter()
                .map(|(pin, node)| (pin.clone(), index_map[node]))
                .collect();
            let ref_base_name = part.part.get_ref_base_name();
            let counter = self.ref_name_counters.entry(ref_base_name).or_insert(1);
            let ref_number = *counter;
            *counter += 1;
            self.parts.push(PartInstance {
                part: part.part,
                ref_number,
                pin_mapping: new_pin_mapping,
                trace: part.trace.prepended(location),
            });
        }
    }
}

impl From<ExpandedCircuit> for BuiltCircuit {
    fn from(expanded: ExpandedCircuit) -> Self {
        BuiltCircuit {
            parts: expanded.parts,
            nets: expanded.nets,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuiltCircuit {
    pub(crate) parts: Vec<PartInstance>,
    pub(crate) nets: NetGraph,
}

impl BuiltCircuit {
    pub fn nets(&self) -> &NetGraph {
        &self.nets
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

    pub fn collect_connected_part_pins(&self) -> Vec<Vec<PinRef>> {
        let connected_nodes = kosaraju_scc(&self.nets);
        let pin_refs = self.pin_refs_by_node();
        connected_nodes.into_iter()
            .map(|nodes| nodes.into_iter().filter_map(|node| pin_refs.get(&node).cloned()).collect())
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
            TestPartPins { pin1: pins["pin1"], pin2: pins["pin2"] }
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

        fn construct_subcircuit(&self, _options: &Self::Options) -> CircuitBuilder<Self::Options> {
            let mut circuit = CircuitBuilder::new();
            let node1 = circuit.add_node();
            let node2 = circuit.add_node();
            let part1_pins = TestPart::instantiate(&mut circuit);
            let part2_pins = TestPart::instantiate(&mut circuit);

            circuit.connect(node1, part1_pins.pin1);
            circuit.connect(part1_pins.pin2, node2);
            circuit.connect(node1, part2_pins.pin1);
            circuit.connect(part2_pins.pin2, node2);

            circuit
        }
    }
    impl TestSubcircuitGenerator1 {
        #[track_caller]
        fn instantiate(circuit: &mut CircuitBuilder) -> TestSubcircuitGenerator1Pins {
            let local_pin1 = NodeIndex::new(0);
            let local_pin2 = NodeIndex::new(1);
            let interface_nodes = HashSet::from([
                local_pin1,
                local_pin2,
            ]);
            let mapping = circuit.instantiate_subcircuit(
                TestSubcircuitGenerator1,
                &interface_nodes,
                |options: &()| *options,
            );
            TestSubcircuitGenerator1Pins { pin1: mapping[&local_pin1], pin2: mapping[&local_pin2] }
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

        fn construct_subcircuit(&self, _options: &Self::Options) -> CircuitBuilder<Self::Options> {
            let mut circuit = CircuitBuilder::new();
            let node1 = circuit.add_node();
            let node2 = circuit.add_node();
            let subcircuit_pins = TestSubcircuitGenerator1::instantiate(&mut circuit);
            let part_pins = TestPart::instantiate(&mut circuit);
            circuit.connect(node1, subcircuit_pins.pin1);
            circuit.connect(subcircuit_pins.pin2, part_pins.pin1);
            circuit.connect(part_pins.pin2, node2);
            circuit
        }
    }
    impl TestSubcircuitGenerator2 {
        #[track_caller]
        fn instantiate(circuit: &mut CircuitBuilder) -> TestSubcircuitGenerator2Pins {
            let local_pin1 = NodeIndex::new(0);
            let local_pin2 = NodeIndex::new(1);
            let interface_nodes = HashSet::from([
                local_pin1,
                local_pin2,
            ]);
            let mapping = circuit.instantiate_subcircuit(
                TestSubcircuitGenerator2,
                &interface_nodes,
                |options: &()| *options,
            );
            TestSubcircuitGenerator2Pins { pin1: mapping[&local_pin1], pin2: mapping[&local_pin2] }
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

        fn construct_subcircuit(&self, _options: &Self::Options) -> CircuitBuilder<Self::Options> {
            let mut circuit = CircuitBuilder::new();
            let interface_nodes =
                HashSet::from([circuit.add_node(), circuit.add_node()]);
            circuit.instantiate_subcircuit(
                LeafOptionsGenerator,
                &interface_nodes,
                |options: &NestedOptions| LeafOptions {
                    part_count: options.part_count,
                },
            );
            circuit
        }
    }

    #[derive(Debug, Clone)]
    struct LeafOptionsGenerator;

    impl SubcircuitGenerator for LeafOptionsGenerator {
        type Options = LeafOptions;

        fn construct_subcircuit(&self, options: &Self::Options) -> CircuitBuilder<Self::Options> {
            let mut circuit = CircuitBuilder::new();
            circuit.add_node();
            circuit.add_node();
            for _ in 0..options.part_count {
                circuit.instantiate_part(Box::new(TestPart));
            }
            circuit
        }
    }

    #[test]
    fn test_generator_specific_options_expand_recursively() {
        let mut circuit = CircuitBuilder::<BuildOptions>::new();
        let interface_nodes = HashSet::from([NodeIndex::new(0), NodeIndex::new(1)]);
        circuit.instantiate_subcircuit(
            NestedOptionsGenerator,
            &interface_nodes,
            |options: &BuildOptions| NestedOptions {
                part_count: options.part_count,
            },
        );

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

        let actual: Vec<Vec<PinRef>> = circuit.collect_connected_part_pins()
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
            vec![
                PinRef {
                    node_index: NodeIndex::new(5),
                    part_ref_name: "U1".to_string(),
                    pin_id: "pin2".to_string(),
                },
            ],
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

        fn construct_subcircuit(&self, _options: &Self::Options) -> CircuitBuilder<Self::Options> {
            let mut circuit = CircuitBuilder::new();
            let (_, line) = (circuit.instantiate_part(Box::new(TestPart)), line!());
            *TRACED_PART_LINE.lock().unwrap() = line;
            circuit
        }
    }

    #[test]
    fn test_traces_follow_subcircuit_instantiations() {
        let mut circuit = CircuitBuilder::new();
        let (_, part_line) = (TestPart::instantiate(&mut circuit), line!());
        #[rustfmt::skip]
        let (_, subcircuit_line) = (circuit.instantiate_subcircuit(TracedSubcircuitGenerator, &HashSet::new(), |o: &()| *o), line!());
        let (node, node_line) = (circuit.add_node(), line!());
        let circuit = circuit.build(&());

        let lines = |trace: &Trace| trace.0.iter().map(|loc| loc.line()).collect::<Vec<_>>();
        assert!(circuit.parts.iter().all(|part| part.trace.0.iter().all(|loc| loc.file() == file!())));
        assert_eq!(lines(&circuit.parts[0].trace), vec![part_line]);
        assert_eq!(
            lines(&circuit.parts[1].trace),
            vec![subcircuit_line, *TRACED_PART_LINE.lock().unwrap()]
        );
        assert_eq!(lines(&circuit.nets[node].trace), vec![node_line]);
        // Pin nodes of a part inside the subcircuit carry the subcircuit trace too.
        let inner_pin = circuit.parts[1].pin_mapping["pin1"];
        assert_eq!(lines(&circuit.nets[inner_pin].trace), lines(&circuit.parts[1].trace));
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
            vec![["GND1", "GND2", "GND3", "unknown"].map(String::from).to_vec()]
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
        assert_eq!(
            groups,
            vec![vec!["GND1", "GND2", "GND3"], vec!["VCC"]]
        );
    }
}
