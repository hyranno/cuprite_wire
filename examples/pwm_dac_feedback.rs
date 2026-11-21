//! Co-simulation: a model of a microcontroller regulates the output of a PWM DAC.
//!
//! ```text
//! MCU pwm --[R 1k]--+-- OUT (to MCU adc)
//!                   |
//!                 [C 100n]
//!                   |
//!                  GND
//! ```
//!
//! The firmware's control loop is written as a Rust model: every 20 us it reads `OUT` with a
//! 12-bit ADC and corrects the duty of a 100 kHz PWM (integral control) so that `OUT` reaches 1 V.
//! ngspice simulates the filter and the pins while the model runs.
//!
//! Needs the ngspice shared library: run with
//! `cargo run --example pwm_dac_feedback --features shared-ngspice`, test with
//! `cargo test --example pwm_dac_feedback --features shared-ngspice`.

#[path = "common/pwm_dac.rs"]
mod pwm_dac;

use cuprite_wire::spice::cosim::{
    DeviceIo, DeviceModel, DeviceModels,
    peripherals::{Adc, Periodic, Pwm},
};
use pwm_dac::bench;

/// The voltage `OUT` is regulated to.
const TARGET: f64 = 1.0;

/// The control loop of the firmware, with the peripherals it uses.
struct Controller {
    pwm: Pwm,
    adc: Adc,
    control: Periodic,
    /// The voltage read at each run of the control loop: (time, volts).
    measured: Vec<(f64, f64)>,
}

impl Controller {
    fn new() -> Self {
        Controller {
            pwm: Pwm::new("pwm", 10e-6, 0.0),
            adc: Adc::new("adc", 12, 3.3),
            control: Periodic::new(20e-6, 0.0),
            measured: Vec::new(),
        }
    }
}

impl DeviceModel for Controller {
    fn step(&mut self, io: &mut DeviceIo<'_>) {
        if self.control.due(io) {
            let measured = self.adc.voltage_of(self.adc.read(io));
            self.measured.push((io.time(), measured));
            self.pwm
                .set_duty(self.pwm.duty() + 0.05 * (TARGET - measured));
        }
        self.pwm.update(io);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let deck = bench(3e-3)?;
    let mut controller = Controller::new();
    deck.run_with(DeviceModels::new().with("mcu", &mut controller))?;
    for &(time, volts) in controller.measured.iter().step_by(10) {
        println!("{:7.0} us  {volts:.3} V", time * 1e6);
    }
    println!("duty: {:.3}", controller.pwm.duty());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `OUT` settles at the target, with the duty at TARGET / 3.3 V.
    #[test]
    fn test_regulates_to_target() {
        let deck = bench(3e-3).unwrap();
        let mut controller = Controller::new();
        let sim = deck
            .run_with(DeviceModels::new().with("mcu", &mut controller))
            .unwrap();
        let &(_, last) = controller.measured.last().unwrap();
        assert!((last - TARGET).abs() < 0.05, "{last}");
        let duty = controller.pwm.duty();
        assert!((duty - TARGET / 3.3).abs() < 0.03, "{duty}");
        // The ripple of the filtered PWM over the last period.
        let table = sim.table("out.txt").unwrap();
        let end = *table.scale().last().unwrap();
        let out = table.column(r#"v("/OUT")"#).unwrap();
        let ripple = table
            .scale()
            .iter()
            .zip(out)
            .filter(|&(&t, _)| t > end - 10e-6)
            .map(|(_, &v)| v);
        let (min, max) = ripple.fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        assert!(min > 0.9 && max < 1.1, "{min} .. {max}");
    }
}
