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
        SpiceDeck,
        cosim::CosimDevice,
        digital::{InputCharacteristics, OutputCharacteristics},
    },
};

/// The microcontroller (`mcu`, a 3.3 V CMOS device with the pins `pwm` and `adc`), the RC filter,
/// and a transient analysis of `stop` seconds writing `OUT` to `out.txt`.
pub fn bench(stop: f64) -> Result<SpiceDeck, Box<dyn std::error::Error>> {
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
    Ok(netlist.deck().control([
        format!("tran 100n {stop:e}"),
        r#"wrdata out.txt v("/OUT")"#.to_string(),
    ]))
}
