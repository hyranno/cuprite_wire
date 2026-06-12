mod parts;
mod subcircuit;

use parts::PartInstance;
use subcircuit::{SubcircuitGenerationOptions, SubcircuitInstance};

use std::{collections::{HashMap, HashSet}, fmt::Debug, panic::Location};

use petgraph::{Graph, graph::NodeIndex};

use crate::{parts::Part, subcircuit::SubcircuitGenerator};

#[track_caller]
pub fn get_location() -> &'static Location<'static> {
    Location::caller()
}

#[derive(Debug)]
pub struct Circuit {
    pub(crate) parts: Vec<PartInstance>,
    pub(crate) ref_name_counters: HashMap<String, u32>,
    pub subcircuits: Vec<SubcircuitInstance>,
    pub nets: Graph<(), &'static Location<'static>>,
}

impl Circuit {
    pub fn new() -> Self {
        Circuit {
            parts: Vec::new(),
            subcircuits: Vec::new(),
            nets: Graph::new(),
            ref_name_counters: HashMap::new(),
        }
    }

    fn merge(
        &mut self,
        other: Circuit,
        overriding_nodes: &HashMap<NodeIndex, NodeIndex>,   // HashMap from other circuit node indices to self circuit node indices for nodes that should be merged
    ) -> HashMap<NodeIndex, NodeIndex> {
        let mut index_map = HashMap::<NodeIndex, NodeIndex>::new();
        for node in other.nets.node_indices() {
            if let Some(&merged_node) = overriding_nodes.get(&node) {
                self.nets[merged_node] = other.nets[node];
                index_map.insert(node, merged_node);
                continue;
            } else {
                let new_index = self.nets.add_node(other.nets[node]);
                index_map.insert(node, new_index);
            }
        }
        for edge in other.nets.edge_indices() {
            let (source, target) = other.nets.edge_endpoints(edge).unwrap();
            self.nets
                .add_edge(index_map[&source], index_map[&target], other.nets.edge_weight(edge).unwrap());
        }
        for part in other.parts {
            let new_pin_mapping = part
                .pin_mapping
                .iter()
                .map(|(pin, node)| (*pin, index_map[node]))
                .collect();
            self.parts.push(PartInstance {
                part: part.part,
                ref_name: part.ref_name,
                pin_mapping: new_pin_mapping,
            });
        }
        for subcircuit in other.subcircuits {
            let new_node_mapping = subcircuit
                .node_mapping
                .iter()
                .map(|(pin, node)| (*pin, index_map[node]))
                .collect();
            self.subcircuits.push(SubcircuitInstance {
                generator: subcircuit.generator,
                node_mapping: new_node_mapping,
            });
        }
        index_map
    }

    pub fn expand_subcircuits(&mut self, options: &SubcircuitGenerationOptions) {
        while !self.subcircuits.is_empty() {
            let instances: Vec<SubcircuitInstance> = self.subcircuits.drain(..).collect();
            for instance in instances {
                let subcircuit =
                    instance
                        .generator
                        .generate_subcircuit(options);
                self.merge(subcircuit, &instance.node_mapping);
            }
        }
    }

    pub fn instantiate_subcircuit(&mut self, generator: Box<dyn SubcircuitGenerator>, interface_nodes: &HashSet<NodeIndex>) -> HashMap<NodeIndex, NodeIndex> {
        let mut node_mapping = HashMap::new();
        for node in interface_nodes {
            node_mapping.insert(*node, self.nets.add_node(()));
        }
        self.subcircuits.push(SubcircuitInstance {
            generator,
            node_mapping: node_mapping.clone(),
        });
        node_mapping
    }

    pub fn instantiate_part(&mut self, part: Box<dyn Part>, num_pins: u32) -> HashMap<u32, NodeIndex> {
        let mut pin_mapping = HashMap::new();
        for pin in 0..num_pins {
            pin_mapping.insert(pin, self.nets.add_node(()));
        }
        let counter = self.ref_name_counters.entry(part.get_ref_base_name()).or_insert(1);
        let ref_name = format!("{}{}", part.get_ref_base_name(), *counter);
        *counter += 1;
        self.parts.push(PartInstance {
            part,
            ref_name,
            pin_mapping: pin_mapping.clone(),
        });
        pin_mapping
    }

}


#[cfg(test)]
mod tests {
    use crate::{parts::Part, subcircuit::SubcircuitGenerator};
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
    }
    impl TestPart {
        fn instantiate(circuit: &mut Circuit) -> TestPartPins {
            let pins = circuit.instantiate_part(Box::new(TestPart), 2);
            TestPartPins { pin1: pins[&0], pin2: pins[&1] }
        }
    }

    #[derive(Debug, Clone)]
    struct TestSubcircuitGenerator1;
    struct TestSubcircuitGenerator1Pins {
        pub pin1: NodeIndex,
        pub pin2: NodeIndex,
    }
    impl SubcircuitGenerator for TestSubcircuitGenerator1 {
        fn generate_subcircuit(&self, _options: &SubcircuitGenerationOptions) -> Circuit {
            let mut circuit = Circuit::new();
            let node1 = circuit.nets.add_node(());
            let node2 = circuit.nets.add_node(());
            circuit.nets.add_edge(node1, node2, get_location());
            circuit
        }
    }
    impl TestSubcircuitGenerator1 {
        fn instantiate(circuit: &mut Circuit) -> TestSubcircuitGenerator1Pins {
            let local_pin1 = NodeIndex::new(0);
            let local_pin2 = NodeIndex::new(1);
            let interface_nodes = HashSet::from([
                local_pin1,
                local_pin2,
            ]);
            let mapping = circuit.instantiate_subcircuit(Box::new(TestSubcircuitGenerator1), &interface_nodes);
            TestSubcircuitGenerator1Pins { pin1: mapping[&local_pin1], pin2: mapping[&local_pin2] }
        }
    }

    #[derive(Debug, Clone)]
    struct TestSubcircuitGenerator2;
    struct TestSubcircuitGenerator2Pins {
        pub pin1: NodeIndex,
        pub pin2: NodeIndex,
    }
    impl SubcircuitGenerator for TestSubcircuitGenerator2 {
        fn generate_subcircuit(&self, _options: &SubcircuitGenerationOptions) -> Circuit {
            let mut circuit = Circuit::new();
            let node1 = circuit.nets.add_node(());
            let node2 = circuit.nets.add_node(());
            let subcircuit_pins1 = TestSubcircuitGenerator1::instantiate(&mut circuit);
            let subcircuit_pins2 = TestSubcircuitGenerator1::instantiate(&mut circuit);
            let part_pins = TestPart::instantiate(&mut circuit);
            circuit.nets.add_edge(subcircuit_pins1.pin2, part_pins.pin1, get_location());
            circuit.nets.add_edge(part_pins.pin2, subcircuit_pins2.pin1, get_location());
            circuit.nets.add_edge(node1, subcircuit_pins1.pin1, get_location());
            circuit.nets.add_edge(node2, subcircuit_pins2.pin2, get_location());
            circuit
        }
    }
    impl TestSubcircuitGenerator2 {
        fn instantiate(circuit: &mut Circuit) -> TestSubcircuitGenerator2Pins {
            let local_pin1 = NodeIndex::new(0);
            let local_pin2 = NodeIndex::new(1);
            let interface_nodes = HashSet::from([
                local_pin1,
                local_pin2,
            ]);
            let mapping = circuit.instantiate_subcircuit(Box::new(TestSubcircuitGenerator2), &interface_nodes);
            TestSubcircuitGenerator2Pins { pin1: mapping[&local_pin1], pin2: mapping[&local_pin2] }
        }
    }


    #[test]
    fn test_expand_subcircuits() {
        let mut circuit = Circuit::new();
        let _subcircuit_pins2 = TestSubcircuitGenerator2::instantiate(&mut circuit);
        // number of nodes before expansion should be 2 (the interface nodes of the top-level subcircuit)
        assert_eq!(circuit.nets.node_count(), 2);
        circuit.expand_subcircuits(&SubcircuitGenerationOptions {});
        // after expansion, we should have 8 nodes: 2 for the top-level subcircuit, 2 for the part, 2 for each of the two subcircuit instances inside it
        assert_eq!(circuit.nets.node_count(), 8);
        // we should have 6 edges: 4 from the top-level subcircuit, and 1 from each of the two subcircuit instances inside it
        assert_eq!(circuit.nets.edge_count(), 6);
        // we should have 0 subcircuit instances remaining after expansion
        assert_eq!(circuit.subcircuits.len(), 0);
        // we should have 1 part instance (the one inside the top-level subcircuit)
        assert_eq!(circuit.parts.len(), 1);
    }
}
