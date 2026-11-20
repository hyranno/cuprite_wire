//! Reusable subcircuits: [`SubcircuitGenerator`] and the [`Interface`] of its nodes.
//!
//! See the guide [Describing circuits](https://github.com/hyranno/cuprite_wire/blob/main/doc/circuits.md#3-nodes-parts-and-subcircuits).

use std::{collections::HashMap, fmt::Debug, marker::PhantomData, panic::Location};

use dyn_clone::DynClone;
use petgraph::graph::NodeIndex;

use crate::{
    circuit::{CircuitBuilder, ExpandedCircuit},
    naming::short_type_name,
    trace::Loc,
};

/// Derives [`Interface`] for a struct with one type parameter whose fields all have that type.
pub use cuprite_wire_macros::Interface;

/// The interface of a subcircuit: the nodes the circuit around it connects to, in a fixed order.
///
/// A subcircuit's interface is usually a struct generic over the type of its fields, deriving this
/// trait. The same struct holds the nodes inside the subcircuit, the nodes of an instance in the
/// circuit around it, and other values per node, such as the pin types given to
/// [`check_subcircuit`](crate::erc::check_subcircuit):
///
/// ```
/// use cuprite_wire::subcircuit::Interface;
/// use petgraph::graph::NodeIndex;
///
/// #[derive(Interface)]
/// pub struct FollowerPorts<T = NodeIndex> {
///     pub input: T,
///     pub output: T,
///     pub vcc: T,
///     pub gnd: T,
/// }
/// ```
///
/// It is also implemented for arrays (`[NodeIndex; 2]`) and for `()` (no interface).
pub trait Interface<T = NodeIndex>: Sized {
    /// The same interface holding `U`s.
    type With<U>: Interface<U>;

    /// Creates the interface from its values in order: `f(0)`, `f(1)`, ...
    fn from_fn(f: impl FnMut(usize) -> T) -> Self;

    /// The values in order.
    fn values(&self) -> Vec<&T>;

    /// The names of the values in order: the field names, or the indices of an array.
    fn names() -> Vec<String>;
}

impl<T, const N: usize> Interface<T> for [T; N] {
    type With<U> = [U; N];

    fn from_fn(f: impl FnMut(usize) -> T) -> Self {
        std::array::from_fn(f)
    }

    fn values(&self) -> Vec<&T> {
        self.iter().collect()
    }

    fn names() -> Vec<String> {
        (0..N).map(|index| index.to_string()).collect()
    }
}

impl<T> Interface<T> for () {
    type With<U> = ();

    fn from_fn(_f: impl FnMut(usize) -> T) -> Self {}

    fn values(&self) -> Vec<&T> {
        Vec::new()
    }

    fn names() -> Vec<String> {
        Vec::new()
    }
}

/// The values of `G`'s interface holding `U`s, e.g. `FollowerPorts<Option<PinType>>`.
pub type InterfaceWith<G, U> = <<G as SubcircuitGenerator>::Interface as Interface>::With<U>;

/// Generates the contents of a subcircuit.
pub trait SubcircuitGenerator: Debug + Clone {
    type Options;
    /// The nodes the circuit around the subcircuit connects to.
    type Interface: Interface;

    /// Adds the contents of the subcircuit to `circuit`, which has only the nodes of `interface`.
    fn construct_subcircuit(
        &self,
        options: &Self::Options,
        circuit: &mut CircuitBuilder<Self::Options>,
        interface: Self::Interface,
    );

    /// Base of automatic instance names (`<base name>#<n>`).
    /// Defaults to the type name without the module path and generic arguments.
    fn base_name(&self) -> String {
        short_type_name::<Self>()
    }
}

/// Creates a circuit with the interface nodes `0..N` of `generator`, traced to `location`, and
/// constructs the subcircuit in it.
pub(crate) fn construct<G: SubcircuitGenerator>(
    generator: &G,
    options: &G::Options,
    location: &'static Location<'static>,
) -> (CircuitBuilder<G::Options>, G::Interface) {
    let mut circuit = CircuitBuilder::new();
    let interface = G::Interface::from_fn(|_| circuit.add_node_at(location));
    let nodes = interface.values().into_iter().copied().collect::<Vec<_>>();
    generator.construct_subcircuit(options, &mut circuit, G::Interface::from_fn(|i| nodes[i]));
    (circuit, interface)
}

pub(crate) trait ErasedSubcircuitGenerator<B>: Debug + DynClone {
    fn generate(&self, options: &B) -> ExpandedCircuit;
}
dyn_clone::clone_trait_object!(<B> ErasedSubcircuitGenerator<B>);

struct BoundSubcircuitGenerator<B, G, F> {
    generator: G,
    options_for_generator: F,
    _builder_options: PhantomData<fn() -> B>,
}

impl<B, G: Clone, F: Clone> Clone for BoundSubcircuitGenerator<B, G, F> {
    fn clone(&self) -> Self {
        Self {
            generator: self.generator.clone(),
            options_for_generator: self.options_for_generator.clone(),
            _builder_options: PhantomData,
        }
    }
}

impl<B, G, F> Debug for BoundSubcircuitGenerator<B, G, F>
where
    G: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundSubcircuitGenerator")
            .field("generator", &self.generator)
            .finish_non_exhaustive()
    }
}

impl<B, G, F> BoundSubcircuitGenerator<B, G, F>
where
    G: SubcircuitGenerator,
{
    fn generate_subcircuit(&self, options: &G::Options) -> ExpandedCircuit {
        let (circuit, _) = construct(&self.generator, options, Location::caller());
        circuit.expand(options)
    }
}

impl<B, G, F> ErasedSubcircuitGenerator<B> for BoundSubcircuitGenerator<B, G, F>
where
    G: SubcircuitGenerator + 'static,
    F: Fn(&B) -> G::Options + Clone + 'static,
{
    fn generate(&self, options: &B) -> ExpandedCircuit {
        let generator_options = (self.options_for_generator)(options);
        self.generate_subcircuit(&generator_options)
    }
}

pub struct SubcircuitInstance<B = ()> {
    pub(crate) generator: Box<dyn ErasedSubcircuitGenerator<B>>,
    pub node_mapping: HashMap<NodeIndex, NodeIndex>,
    /// Explicit or automatic instance name.
    pub name: String,
    /// Where the subcircuit was instantiated.
    pub location: Loc,
}

impl<B> Clone for SubcircuitInstance<B> {
    fn clone(&self) -> Self {
        Self {
            generator: dyn_clone::clone_box(&*self.generator),
            node_mapping: self.node_mapping.clone(),
            name: self.name.clone(),
            location: self.location,
        }
    }
}

impl<B> Debug for SubcircuitInstance<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubcircuitInstance")
            .field("node_mapping", &self.node_mapping)
            .field("name", &self.name)
            .field("location", &self.location)
            .finish_non_exhaustive()
    }
}

impl<B> SubcircuitInstance<B> {
    pub(crate) fn new<G, F>(
        generator: G,
        options_for_generator: F,
        node_mapping: HashMap<NodeIndex, NodeIndex>,
        name: String,
        location: Loc,
    ) -> Self
    where
        B: 'static,
        G: SubcircuitGenerator + 'static,
        F: Fn(&B) -> G::Options + Clone + 'static,
    {
        Self {
            generator: Box::new(BoundSubcircuitGenerator {
                generator,
                options_for_generator,
                _builder_options: PhantomData,
            }),
            node_mapping,
            name,
            location,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Interface, Debug, PartialEq)]
    struct Ports<T = NodeIndex> {
        input: T,
        output: T,
    }

    #[test]
    fn test_derived_interface() {
        let ports = <Ports>::from_fn(NodeIndex::new);
        assert_eq!(
            ports,
            Ports {
                input: NodeIndex::new(0),
                output: NodeIndex::new(1)
            }
        );
        assert_eq!(ports.values(), [&ports.input, &ports.output]);
        assert_eq!(<Ports as Interface>::names(), ["input", "output"]);
        let mut labels = ["a", "b"].into_iter();
        let labels: <Ports as Interface>::With<&str> =
            Interface::from_fn(|_| labels.next().unwrap());
        assert_eq!(labels.values(), [&"a", &"b"]);
    }

    #[test]
    fn test_array_and_unit_interfaces() {
        let nodes = <[NodeIndex; 3]>::from_fn(NodeIndex::new);
        assert_eq!(nodes, [0, 1, 2].map(NodeIndex::new));
        assert_eq!(nodes.values(), nodes.iter().collect::<Vec<_>>());
        assert_eq!(<[NodeIndex; 3] as Interface>::names(), ["0", "1", "2"]);
        <() as Interface>::from_fn(|_| unreachable!());
        assert!(Interface::<NodeIndex>::values(&()).is_empty());
        assert!(<() as Interface>::names().is_empty());
    }
}
