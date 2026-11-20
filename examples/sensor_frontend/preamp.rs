//! AC-coupled non-inverting preamplifier, biased at the reference.

use cuprite_wire::{
    basic::{Capacitor0603, Resistor0603},
    circuit::CircuitBuilder,
    subcircuit::SubcircuitGenerator,
};
use petgraph::graph::NodeIndex;

use crate::parts::RrioOpAmp;

/// Gain `1 + feedback / ground` above the input high-pass corner (1.6 Hz with 1u and 100k).
/// The output sits at `vref` without a signal.
///
/// Interface: `[input, vref, vcc, gnd, output]`.
#[derive(Debug, Clone)]
pub struct Preamp {
    /// The feedback resistor, from the output to the inverting input.
    pub feedback: &'static str,
    /// The gain-setting resistor, from the inverting input to `vref`.
    pub ground: &'static str,
}

impl Default for Preamp {
    /// A gain of 11 (20.8 dB).
    fn default() -> Self {
        Self {
            feedback: "100k",
            ground: "10k",
        }
    }
}

pub struct PreampPorts {
    pub input: NodeIndex,
    pub vref: NodeIndex,
    pub vcc: NodeIndex,
    pub gnd: NodeIndex,
    pub output: NodeIndex,
}

impl Preamp {
    #[track_caller]
    pub fn place(self, circuit: &mut CircuitBuilder, name: &str) -> PreampPorts {
        let [input, vref, vcc, gnd, output] =
            circuit.instantiate_subcircuit_interface_named(name, self, |o: &()| *o);
        PreampPorts {
            input,
            vref,
            vcc,
            gnd,
            output,
        }
    }
}

impl SubcircuitGenerator for Preamp {
    type Options = ();

    fn construct_subcircuit(&self, _options: &()) -> CircuitBuilder {
        let mut c = CircuitBuilder::new();
        let [input, vref, vcc, gnd, output] = c.add_interface();
        let coupling = c.instantiate_part_named("c_in", Box::new(Capacitor0603::new("1u")));
        let bias = c.instantiate_part_named("r_bias", Box::new(Resistor0603::new("100k")));
        let feedback =
            c.instantiate_part_named("r_feedback", Box::new(Resistor0603::new(self.feedback)));
        let ground = c.instantiate_part_named("r_ground", Box::new(Resistor0603::new(self.ground)));
        let decoupling =
            c.instantiate_part_named("c_decoupling", Box::new(Capacitor0603::new("100n")));
        let amp = c.instantiate_part_named("amp", Box::new(RrioOpAmp::new()));
        // Input coupling, biased at the reference.
        c.connect(input, coupling["1"]);
        c.connect(coupling["2"], amp[RrioOpAmp::IN_P]);
        c.connect(amp[RrioOpAmp::IN_P], bias["1"]);
        c.connect(bias["2"], vref);
        // Gain.
        c.connect(amp[RrioOpAmp::OUT], feedback["1"]);
        c.connect(feedback["2"], amp[RrioOpAmp::IN_N]);
        c.connect(amp[RrioOpAmp::IN_N], ground["1"]);
        c.connect(ground["2"], vref);
        c.connect(amp[RrioOpAmp::OUT], output);
        // Supply.
        c.connect(vcc, amp[RrioOpAmp::VCC]);
        c.connect(gnd, amp[RrioOpAmp::VEE]);
        c.connect(vcc, decoupling["1"]);
        c.connect(gnd, decoupling["2"]);
        c
    }
}

#[cfg(test)]
mod tests {
    use cuprite_wire::{
        erc::{ErcSettings, check_subcircuit},
        parts::PinType,
        spice::{IndependentSource, Waveform},
        subcircuit::interface,
    };

    use super::*;
    use crate::bench::{Bench, assert_near, value};

    #[test]
    fn erc() {
        let [input, vref, vcc, gnd, output] = interface();
        let ports = [
            (input, PinType::Output),
            (vref, PinType::Output),
            (vcc, PinType::PowerOut),
            (gnd, PinType::PowerOut),
            (output, PinType::Input),
        ];
        check_subcircuit(&Preamp::default(), &(), &ports, &ErcSettings::default())
            .unwrap()
            .assert_clean();
    }

    /// The preamp on a 5 V supply with an ideal 2.5 V reference, driven by `input`.
    fn bench(input: Box<IndependentSource>) -> Bench {
        let mut bench = Bench::new(IndependentSource::voltage(5.0));
        let ports = Preamp::default().place(&mut bench.circuit, "preamp");
        bench.circuit.connect(ports.vcc, bench.vcc);
        bench.circuit.connect(ports.gnd, bench.gnd);
        let vref = bench.source("vref", IndependentSource::voltage(2.5));
        bench.circuit.connect(ports.vref, vref);
        let input = bench.source("input", input);
        bench.circuit.connect(ports.input, input);
        bench.probe(ports.output, "OUT");
        bench
    }

    #[test]
    fn gain_and_high_pass() {
        let bench = bench(IndependentSource::voltage(0.0).with_ac(1.0));
        let sim = bench.simulate([
            "ac dec 20 0.1 100k",
            r#"meas ac gain FIND vdb("/OUT") AT=1k"#,
            r#"meas ac f_low WHEN vdb("/OUT")=17.8 RISE=1"#,
        ]);
        assert_near("gain (dB)", value(&sim, "gain"), 20.0 * 11f64.log10(), 0.05);
        // 1 / (2 pi 100k 1u) = 1.59 Hz.
        assert_near("f_low (Hz)", value(&sim, "f_low"), 1.59, 0.1);
    }

    /// A 10 mV amplitude input gives 110 mV around the reference (this also fails if the
    /// feedback were positive, which the AC analysis alone does not show).
    #[test]
    fn small_signal() {
        let bench =
            bench(IndependentSource::voltage(0.0).with_waveform(Waveform::sin(0.0, 0.01, 1e3)));
        let sim = bench.simulate([
            "tran 1u 3m 1m",
            r#"meas tran vmax MAX v("/OUT")"#,
            r#"meas tran vmin MIN v("/OUT")"#,
        ]);
        assert_near("vmax", value(&sim, "vmax"), 2.5 + 0.11, 0.003);
        assert_near("vmin", value(&sim, "vmin"), 2.5 - 0.11, 0.003);
    }

    /// A 0.4 V amplitude input would give 4.4 V; the output clips at the rails.
    #[test]
    fn clips_at_the_rails() {
        let bench =
            bench(IndependentSource::voltage(0.0).with_waveform(Waveform::sin(0.0, 0.4, 1e3)));
        let sim = bench.simulate([
            "tran 1u 3m 1m",
            r#"meas tran vmax MAX v("/OUT")"#,
            r#"meas tran vmin MIN v("/OUT")"#,
        ]);
        assert_near("vmax", value(&sim, "vmax"), 4.99, 0.02);
        assert_near("vmin", value(&sim, "vmin"), 0.01, 0.02);
    }
}
