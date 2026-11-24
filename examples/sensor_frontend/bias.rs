//! Mid-supply reference: the AC ground of the single-supply signal chain.

use cuprite_wire::{
    basic::{Capacitor0603, Resistor0603},
    circuit::{CircuitBuilder, Named, Node},
    subcircuit::{Interface, SubcircuitGenerator},
};

use crate::parts::RrioOpAmp;

/// Divides the supply by two, filters it and buffers it with an op-amp.
#[derive(Debug, Clone)]
pub struct MidSupply;

/// The interface of [`MidSupply`].
#[derive(Interface)]
pub struct MidSupplyPorts<T = Node> {
    pub vcc: T,
    pub gnd: T,
    pub vref: T,
}

impl SubcircuitGenerator for MidSupply {
    type Options = ();
    type Interface = MidSupplyPorts;

    fn construct_subcircuit(
        &self,
        _options: &(),
        c: &mut CircuitBuilder,
        interface: Self::Interface,
    ) {
        let MidSupplyPorts { vcc, gnd, vref } = interface;
        let upper = c.instantiate_part(Named("r_upper", Resistor0603::new("100k")));
        let lower = c.instantiate_part(Named("r_lower", Resistor0603::new("100k")));
        let filter = c.instantiate_part(Named("c_filter", Capacitor0603::new("1u")));
        let decoupling = c.instantiate_part(Named("c_decoupling", Capacitor0603::new("100n")));
        let amp = c.instantiate_part(Named("buffer", RrioOpAmp::new()));
        // Divider, filtered (corner 3.2 Hz with 50k and 1u).
        c.connect(vcc, upper["1"]);
        c.connect(upper["2"], lower["1"]);
        c.connect(lower["2"], gnd);
        c.connect(upper["2"], filter["1"]);
        c.connect(filter["2"], gnd);
        // Follower.
        c.connect(upper["2"], amp[RrioOpAmp::IN_P]);
        c.connect(amp[RrioOpAmp::OUT], amp[RrioOpAmp::IN_N]);
        c.connect(amp[RrioOpAmp::OUT], vref);
        // Supply.
        c.connect(vcc, amp[RrioOpAmp::VCC]);
        c.connect(gnd, amp[RrioOpAmp::VEE]);
        c.connect(vcc, decoupling["1"]);
        c.connect(gnd, decoupling["2"]);
    }
}

#[cfg(test)]
mod tests {
    use cuprite_wire::{
        erc::{ErcSettings, check_subcircuit},
        parts::PinType,
        spice::{IndependentSource, Waveform, analysis::Tran},
    };

    use super::*;
    use crate::bench::{Bench, assert_near, run};

    #[test]
    fn erc() {
        let ports = MidSupplyPorts {
            vcc: Some(PinType::PowerOut),
            gnd: Some(PinType::PowerOut),
            vref: Some(PinType::Input),
        };
        check_subcircuit(&MidSupply, &(), &ports, &ErcSettings::default())
            .unwrap()
            .assert_clean();
    }

    /// After power-up the reference settles at half the supply, also under load.
    ///
    /// A transient analysis rather than an operating point: with the op-amp inputs swapped, the
    /// operating point would still find the (unstable) balance, while the transient runs away.
    #[test]
    fn half_supply() {
        for supply in [3.3, 5.0] {
            // The supply ramps up in 1 ms.
            let ramp = Waveform::Pwl(vec![(0.0, 0.0), (1e-3, supply)]);
            let mut bench = Bench::new(IndependentSource::voltage(0.0).with_waveform(ramp));
            let ports = bench
                .circuit
                .instantiate_subcircuit(Named("bias", MidSupply));
            bench.circuit.connect(ports.vcc, bench.vcc);
            bench.circuit.connect(ports.gnd, bench.gnd);
            bench.probe(ports.vref, "VREF");
            // A 1 mA load, as if other stages were biased from the reference.
            let load = bench
                .circuit
                .instantiate_part(Named("load", Resistor0603::new("2.5k")));
            bench.circuit.connect(load["1"], ports.vref);
            bench.circuit.connect(load["2"], bench.gnd);
            // The divider's filter settles with a time constant of 50 ms.
            let deck = bench.deck();
            let vref = deck.voltage("/VREF").unwrap();
            let tran = Tran::new(100e-6, 600e-3).probe(&vref);
            let result = run(&deck.analysis(&tran)).result(&tran).unwrap();
            let last = *result.get(&vref).unwrap().last().unwrap();
            assert_near("vref", last, supply / 2.0, 1e-3);
        }
    }
}
