//! The test bench of the PWM DAC examples: `examples/pwm_dac_feedback.rs` and
//! `crates/embedded_hal/examples/pwm_dac_firmware.rs` (which includes this file by its path).
//!
//! ```text
//! MCU pwm --[R 1k]--+-- OUT (to MCU adc)
//!                   |
//!                 [C 100n]
//!                   |
//!                  GND
//! ```

use cuprite_wire::{
    basic::{Capacitor0603, Resistor0603},
    circuit::{CircuitBuilder, Named},
    naming::NetLabel,
    spice::{
        Simulation, SpiceDeck,
        analysis::{Probe, Tran},
        cosim::{CosimDevice, DeviceModel, DeviceModels},
        digital::{InputCharacteristics, OutputCharacteristics},
    },
};

/// The deck of the bench with its transient analysis and the probe of `OUT`.
pub struct Bench {
    pub deck: SpiceDeck,
    /// The transient analysis recording `out`.
    pub tran: Tran,
    /// The voltage of `OUT`.
    pub out: Probe,
}

impl Bench {
    /// Runs the bench with `mcu` simulating the microcontroller; returns the simulation and the
    /// voltage of `OUT` at the end.
    pub fn run(
        &self,
        mcu: &mut dyn DeviceModel,
    ) -> Result<(Simulation, f64), Box<dyn std::error::Error>> {
        let sim = self.deck.run_with(DeviceModels::new().with("mcu", mcu))?;
        let tran = sim.result(&self.tran)?;
        let end = *tran.scale().last().ok_or("no time points")?;
        let out = tran.interpolate(&self.out, end)?.ok_or("no OUT")?;
        Ok((sim, out))
    }
}

/// The microcontroller (`mcu`, a 3.3 V CMOS device with the pins `pwm` and `adc`), the RC filter,
/// and a transient analysis of `stop` seconds recording `OUT`.
pub fn bench(stop: f64) -> Result<Bench, Box<dyn std::error::Error>> {
    let mcu = CosimDevice::new("mcu")
        .output("pwm", OutputCharacteristics::cmos(3.3))
        .input("adc", InputCharacteristics::cmos(3.3));
    let mut bench = CircuitBuilder::new();
    let mcu = bench.instantiate_part(Named("mcu", mcu));
    let r = bench.instantiate_part(Named("r", Resistor0603::new("1k")));
    let c = bench.instantiate_part(Named("c", Capacitor0603::new("100n")));
    bench.connect(mcu["pwm"], r["1"]);
    bench.connect(r["2"], c["1"]);
    bench.connect(c["1"], mcu["adc"]);
    bench.label(c["1"], NetLabel::local("OUT"));
    bench.label(c["2"], NetLabel::global("GND"));
    bench.label(mcu[CosimDevice::GND], NetLabel::global("GND"));
    let netlist = bench.build(&()).to_netlist()?.to_spice()?;
    let deck = netlist.deck();
    let out = deck.voltage("/OUT")?;
    let tran = Tran::new(100e-9, stop).probe(&out);
    Ok(Bench {
        deck: deck.analysis(&tran),
        tran,
        out,
    })
}
