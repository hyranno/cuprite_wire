//! Anti-aliasing low-pass filter in front of the ADC.

use cuprite_wire::{
    basic::{Capacitor0603, Resistor0603},
    circuit::CircuitBuilder,
    subcircuit::{Interface, SubcircuitGenerator},
};
use petgraph::graph::NodeIndex;

use crate::parts::RrioOpAmp;

/// Unity-gain Sallen-Key second-order low-pass filter: 10k, 10k, 22n (feedback) and 10n (to
/// `vref`), a natural frequency of 1.07 kHz and a Q of 0.74 (about Butterworth).
#[derive(Debug, Clone)]
pub struct LowPass;

/// The interface of [`LowPass`].
#[derive(Interface)]
pub struct LowPassPorts<T = NodeIndex> {
    pub input: T,
    pub vref: T,
    pub vcc: T,
    pub gnd: T,
    pub output: T,
}

impl SubcircuitGenerator for LowPass {
    type Options = ();
    type Interface = LowPassPorts;

    fn construct_subcircuit(
        &self,
        _options: &(),
        c: &mut CircuitBuilder,
        interface: Self::Interface,
    ) {
        let LowPassPorts {
            input,
            vref,
            vcc,
            gnd,
            output,
        } = interface;
        let r1 = c.instantiate_part_named("r1", Box::new(Resistor0603::new("10k")));
        let r2 = c.instantiate_part_named("r2", Box::new(Resistor0603::new("10k")));
        let c_feedback =
            c.instantiate_part_named("c_feedback", Box::new(Capacitor0603::new("22n")));
        let c_ground = c.instantiate_part_named("c_ground", Box::new(Capacitor0603::new("10n")));
        let decoupling =
            c.instantiate_part_named("c_decoupling", Box::new(Capacitor0603::new("100n")));
        let amp = c.instantiate_part_named("amp", Box::new(RrioOpAmp::new()));
        c.connect(input, r1["1"]);
        c.connect(r1["2"], r2["1"]);
        c.connect(r2["2"], amp[RrioOpAmp::IN_P]);
        c.connect(r1["2"], c_feedback["1"]);
        c.connect(c_feedback["2"], output);
        c.connect(amp[RrioOpAmp::IN_P], c_ground["1"]);
        c.connect(c_ground["2"], vref);
        c.connect(amp[RrioOpAmp::OUT], amp[RrioOpAmp::IN_N]);
        c.connect(amp[RrioOpAmp::OUT], output);
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
        spice::{IndependentSource, Waveform},
    };

    use super::*;
    use crate::bench::{Bench, StageNodes, assert_near};

    #[test]
    fn erc() {
        let ports = LowPassPorts {
            input: Some(PinType::Output),
            vref: Some(PinType::Output),
            vcc: Some(PinType::PowerOut),
            gnd: Some(PinType::PowerOut),
            output: Some(PinType::Input),
        };
        check_subcircuit(&LowPass, &(), &ports, &ErcSettings::default())
            .unwrap()
            .assert_clean();
    }

    /// The filter on a 5 V supply with an ideal 2.5 V reference, driven by `input`.
    fn bench(input: Box<IndependentSource>) -> Bench {
        Bench::stage(input, |circuit| {
            let LowPassPorts {
                input,
                vref,
                vcc,
                gnd,
                output,
            } = circuit.instantiate_subcircuit_named("filter", LowPass);
            StageNodes {
                input,
                vref,
                vcc,
                gnd,
                output,
            }
        })
    }

    #[test]
    fn frequency_response() {
        let bench = bench(IndependentSource::voltage(2.5).with_ac(1.0));
        let sim = bench.simulate([
            "op",
            r#"let vout = v("/OUT")"#,
            "print vout",
            "ac dec 50 10 100k",
            r#"meas ac pass FIND vdb("/OUT") AT=100"#,
            r#"meas ac f3db WHEN vdb("/OUT")=-3"#,
            r#"meas ac stop FIND vdb("/OUT") AT=10k"#,
        ]);
        // The output follows the input at DC.
        assert_near("vout", sim.expect_value("vout"), 2.5, 1e-3);
        assert_near("pass (dB)", sim.expect_value("pass"), 0.0, 0.1);
        // With Q = 0.74 the -3 dB point is a little above the natural frequency.
        assert_near("f3db (Hz)", sim.expect_value("f3db"), 1120.0, 50.0);
        // -40 dB/decade: about -40 dB one decade above the corner.
        let stop = sim.expect_value("stop");
        assert!(stop < -38.0, "stop = {stop} dB");
    }

    /// A 100 mV step settles with a small overshoot (5.7 % for Q = 0.74).
    #[test]
    fn step_response() {
        let step = Waveform::Pulse {
            initial: 2.5,
            pulsed: 2.6,
            delay: 1e-3,
            rise: 1e-6,
            fall: 1e-6,
            width: 1.0,
            period: 2.0,
        };
        let bench = bench(IndependentSource::voltage(2.5).with_waveform(step));
        let sim = bench.simulate([
            "tran 1u 5m",
            r#"meas tran vpeak MAX v("/OUT") FROM=1m TO=5m"#,
            r#"meas tran vfinal FIND v("/OUT") AT=5m"#,
        ]);
        assert_near("vfinal", sim.expect_value("vfinal"), 2.6, 1e-3);
        assert_near("overshoot", sim.expect_value("vpeak") - 2.6, 0.0057, 0.001);
    }
}
