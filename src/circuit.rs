use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    panic::Location,
};

use petgraph::{
    algo::kosaraju_scc,
    graph::{NodeIndex, UnGraph},
};

use crate::{
    parts::{Part, PartInstance, PinRef},
    subcircuit::{SubcircuitGenerator, SubcircuitInstance},
};

pub struct CircuitBuilder<B = ()> {
    pub(crate) parts: Vec<PartInstance>,
    pub(crate) ref_name_counters: HashMap<String, u32>,
    pub subcircuits: Vec<SubcircuitInstance<B>>,
    pub nets: UnGraph<(), &'static Location<'static>>,
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
            nets: UnGraph::new_undirected(),
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
            expanded.merge(subcircuit, &instance.node_mapping);
        }

        expanded
    }

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
        let mut node_mapping = HashMap::new();
        for node in interface_nodes {
            node_mapping.insert(*node, self.nets.add_node(()));
        }
        self.subcircuits.push(SubcircuitInstance::new(
            generator,
            options_for_generator,
            node_mapping.clone(),
        ));
        node_mapping
    }

    pub fn instantiate_part(&mut self, part: Box<dyn Part>) -> HashMap<String, NodeIndex> {
        let mut pin_mapping = HashMap::new();
        for pin in part.pin_ids() {
            pin_mapping.insert(pin, self.nets.add_node(()));
        }
        let ref_base_name = part.get_ref_base_name();
        let counter = self.ref_name_counters.entry(ref_base_name).or_insert(1);
        let ref_number = *counter;
        *counter += 1;
        self.parts.push(PartInstance {
            part,
            ref_number,
            pin_mapping: pin_mapping.clone(),
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
    nets: UnGraph<(), &'static Location<'static>>,
}

impl ExpandedCircuit {
    fn merge(&mut self, other: ExpandedCircuit, overriding_nodes: &HashMap<NodeIndex, NodeIndex>) {
        let mut index_map = HashMap::<NodeIndex, NodeIndex>::new();
        for node in other.nets.node_indices() {
            if let Some(&merged_node) = overriding_nodes.get(&node) {
                self.nets[merged_node] = other.nets[node];
                index_map.insert(node, merged_node);
            } else {
                let new_index = self.nets.add_node(());
                index_map.insert(node, new_index);
            }
        }

        for edge in other.nets.edge_indices() {
            let (source, target) = other.nets.edge_endpoints(edge).unwrap();
            self.nets.add_edge(
                index_map[&source],
                index_map[&target],
                *other.nets.edge_weight(edge).unwrap(),
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
    parts: Vec<PartInstance>,
    nets: UnGraph<(), &'static Location<'static>>,
}

impl BuiltCircuit {
    pub fn nets(&self) -> &UnGraph<(), &'static Location<'static>> {
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
        get_location,
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
    }
    impl TestPart {
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
            let node1 = circuit.nets.add_node(());
            let node2 = circuit.nets.add_node(());
            let part1_pins = TestPart::instantiate(&mut circuit);
            let part2_pins = TestPart::instantiate(&mut circuit);

            circuit.nets.add_edge(node1, part1_pins.pin1, get_location());
            circuit.nets.add_edge(part1_pins.pin2, node2, get_location());
            circuit.nets.add_edge(node1, part2_pins.pin1, get_location());
            circuit.nets.add_edge(part2_pins.pin2, node2, get_location());

            circuit
        }
    }
    impl TestSubcircuitGenerator1 {
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
            let node1 = circuit.nets.add_node(());
            let node2 = circuit.nets.add_node(());
            let subcircuit_pins = TestSubcircuitGenerator1::instantiate(&mut circuit);
            let part_pins = TestPart::instantiate(&mut circuit);
            circuit.nets.add_edge(node1, subcircuit_pins.pin1, get_location());
            circuit.nets.add_edge(subcircuit_pins.pin2, part_pins.pin1, get_location());
            circuit.nets.add_edge(part_pins.pin2, node2, get_location());
            circuit
        }
    }
    impl TestSubcircuitGenerator2 {
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
                HashSet::from([circuit.nets.add_node(()), circuit.nets.add_node(())]);
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
            circuit.nets.add_node(());
            circuit.nets.add_node(());
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
}