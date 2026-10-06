use std::{collections::HashMap, fmt::Debug, marker::PhantomData};

use dyn_clone::DynClone;
use petgraph::graph::NodeIndex;

use crate::circuit::{CircuitBuilder, ExpandedCircuit};

pub trait SubcircuitGenerator: Debug + Clone {
    type Options;

    fn construct_subcircuit(&self, options: &Self::Options) -> CircuitBuilder<Self::Options>;
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
        self.generator.construct_subcircuit(options).expand(options)
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
}

impl<B> Clone for SubcircuitInstance<B> {
    fn clone(&self) -> Self {
        Self {
            generator: dyn_clone::clone_box(&*self.generator),
            node_mapping: self.node_mapping.clone(),
        }
    }
}

impl<B> Debug for SubcircuitInstance<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubcircuitInstance")
            .field("node_mapping", &self.node_mapping)
            .finish_non_exhaustive()
    }
}

impl<B> SubcircuitInstance<B> {
    pub(crate) fn new<G, F>(
        generator: G,
        options_for_generator: F,
        node_mapping: HashMap<NodeIndex, NodeIndex>,
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
        }
    }
}
