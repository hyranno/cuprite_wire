//! Reusable subcircuits: [`SubcircuitGenerator`] and the [`Interface`] of its nodes.
//!
//! See the guide [Describing circuits](https://github.com/hyranno/cuprite_wire/blob/main/doc/circuits.md#3-nodes-parts-and-subcircuits).

use std::{fmt::Debug, marker::PhantomData};

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
/// use cuprite_wire::{circuit::NodeIndex, subcircuit::Interface};
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
    /// The options the subcircuit is built with, given to [`construct_subcircuit`] and to the
    /// generators of the subcircuits inside it. Use `()` if there are none.
    ///
    /// [`construct_subcircuit`]: SubcircuitGenerator::construct_subcircuit
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
    /// It must not be empty or contain `#` or `{`, like an explicit name.
    fn base_name(&self) -> String {
        short_type_name::<Self>()
    }

    /// This generator, for a circuit built with options of another type `B`:
    /// `options_for_generator` derives its options from them. Pass the result to
    /// [`CircuitBuilder::instantiate_subcircuit`].
    fn with_options<B, F>(self, options_for_generator: F) -> WithOptions<Self, F>
    where
        F: Fn(&B) -> Self::Options + Clone,
    {
        WithOptions {
            generator: self,
            options_for_generator,
        }
    }
}

/// The nodes of `interface` in order.
pub(crate) fn interface_nodes(interface: &impl Interface) -> Vec<NodeIndex> {
    interface.values().into_iter().copied().collect()
}

/// Creates a circuit with the interface nodes of `generator`, traced to `location`, and
/// constructs the subcircuit in it. Returns it with its interface nodes in order.
pub(crate) fn construct<G: SubcircuitGenerator>(
    generator: &G,
    options: &G::Options,
    location: Loc,
) -> (CircuitBuilder<G::Options>, Vec<NodeIndex>) {
    let mut circuit = CircuitBuilder::new();
    let interface = G::Interface::from_fn(|_| circuit.add_node_at(location));
    let nodes = interface_nodes(&interface);
    generator.construct_subcircuit(options, &mut circuit, interface);
    (circuit, nodes)
}

pub(crate) trait ErasedSubcircuitGenerator<B>: Debug + DynClone {
    /// Generates the subcircuit for an instance at `location`. Returns it with its interface
    /// nodes in order.
    fn generate(&self, options: &B, location: Loc) -> (ExpandedCircuit, Vec<NodeIndex>);
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

impl<B, G, F> ErasedSubcircuitGenerator<B> for BoundSubcircuitGenerator<B, G, F>
where
    G: SubcircuitGenerator + 'static,
    F: Fn(&B) -> G::Options + Clone + 'static,
{
    fn generate(&self, options: &B, location: Loc) -> (ExpandedCircuit, Vec<NodeIndex>) {
        let options = (self.options_for_generator)(options);
        let (circuit, interface) = construct(&self.generator, &options, location);
        (circuit.expand(&options), interface)
    }
}

mod bound {
    use super::ErasedSubcircuitGenerator;

    /// A generator bound to the options of the circuit it is instantiated in (`B`), with its base
    /// name. Not nameable outside the crate.
    pub struct BoundGenerator<B> {
        pub(crate) generator: Box<dyn ErasedSubcircuitGenerator<B>>,
        pub(crate) base_name: String,
    }
}
pub(crate) use bound::BoundGenerator;

impl<B> BoundGenerator<B> {
    fn new<G, F>(generator: G, options_for_generator: F) -> Self
    where
        B: 'static,
        G: SubcircuitGenerator + 'static,
        F: Fn(&B) -> G::Options + Clone + 'static,
    {
        BoundGenerator {
            base_name: generator.base_name(),
            generator: Box::new(BoundSubcircuitGenerator {
                generator,
                options_for_generator,
                _builder_options: PhantomData,
            }),
        }
    }
}

/// What [`CircuitBuilder::instantiate_subcircuit`] takes in a circuit built with options of type
/// `B`: a [`SubcircuitGenerator`] with the options `B`, one with its own options
/// ([`SubcircuitGenerator::with_options`]), or either [`Named`](crate::circuit::Named).
pub trait IntoSubcircuit<B> {
    /// The interface of the subcircuit.
    type Interface: Interface;

    /// The explicit instance name, if any, and the bound generator.
    #[doc(hidden)]
    fn into_named_subcircuit(self) -> (Option<String>, BoundGenerator<B>);
}

impl<B, G> IntoSubcircuit<B> for G
where
    B: Clone + 'static,
    G: SubcircuitGenerator<Options = B> + 'static,
{
    type Interface = G::Interface;

    fn into_named_subcircuit(self) -> (Option<String>, BoundGenerator<B>) {
        (None, BoundGenerator::new(self, B::clone))
    }
}

/// A [`SubcircuitGenerator`] whose options are derived from the options of the circuit it is
/// instantiated in, made by [`SubcircuitGenerator::with_options`].
#[derive(Clone)]
pub struct WithOptions<G, F> {
    generator: G,
    options_for_generator: F,
}

impl<G: Debug, F> Debug for WithOptions<G, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WithOptions")
            .field("generator", &self.generator)
            .finish_non_exhaustive()
    }
}

impl<B, G, F> IntoSubcircuit<B> for WithOptions<G, F>
where
    B: 'static,
    G: SubcircuitGenerator + 'static,
    F: Fn(&B) -> G::Options + Clone + 'static,
{
    type Interface = G::Interface;

    fn into_named_subcircuit(self) -> (Option<String>, BoundGenerator<B>) {
        (
            None,
            BoundGenerator::new(self.generator, self.options_for_generator),
        )
    }
}

/// A subcircuit instantiated in a [`CircuitBuilder`], expanded when the circuit is built with
/// options of type `B`.
pub(crate) struct SubcircuitInstance<B = ()> {
    pub(crate) generator: Box<dyn ErasedSubcircuitGenerator<B>>,
    /// The nodes in the circuit around the instance, connected to the subcircuit's interface nodes
    /// in order.
    pub(crate) interface: Vec<NodeIndex>,
    /// Explicit or automatic instance name.
    pub(crate) name: String,
    /// Where the subcircuit was instantiated.
    pub(crate) location: Loc,
}

impl<B> Clone for SubcircuitInstance<B> {
    fn clone(&self) -> Self {
        Self {
            generator: dyn_clone::clone_box(&*self.generator),
            interface: self.interface.clone(),
            name: self.name.clone(),
            location: self.location,
        }
    }
}

impl<B> Debug for SubcircuitInstance<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubcircuitInstance")
            .field("interface", &self.interface)
            .field("name", &self.name)
            .field("location", &self.location)
            .finish_non_exhaustive()
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
