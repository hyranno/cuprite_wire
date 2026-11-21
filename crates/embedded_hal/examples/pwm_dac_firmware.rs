//! The control loop of `examples/pwm_dac_feedback.rs` (in the cuprite_wire crate) written as
//! firmware code: [`firmware`] depends only on `core` and the `embedded-hal` traits, as it would
//! in the firmware crate built for the microcontroller, and runs unchanged against the simulated
//! RC-filtered PWM DAC through the pins of a [`Board`].
//!
//! Run with `cargo run -p cuprite_wire_embedded_hal --example pwm_dac_firmware`, test with
//! `cargo test -p cuprite_wire_embedded_hal --example pwm_dac_firmware` (both need the ngspice
//! shared library).

/// The same test bench as `pwm_dac_feedback`.
#[path = "../../../examples/common/pwm_dac.rs"]
mod pwm_dac;

use cuprite_wire::spice::cosim::DeviceModels;
use cuprite_wire_embedded_hal::{AdcChannel, AdcPin, Board, Firmware, PwmChannel};
use pwm_dac::bench;

/// The firmware's control loop, as in the firmware crate (`#![no_std]` there).
mod firmware {
    use embedded_hal::pwm::SetDutyCycle;

    /// The firmware's own ADC trait (`embedded-hal` 1.0 has none), implemented by its HAL on the
    /// target and by the test bench in the simulation.
    pub trait AnalogInput {
        /// A 12-bit code, full scale at 3.3 V.
        fn read(&mut self) -> u16;
    }

    /// The code of the target voltage: 1 V of 3.3 V in 12 bits.
    pub const TARGET: u16 = 1241;

    /// Integral control of a PWM DAC; `step` runs every 20 us.
    pub struct Regulator<P, A> {
        pub pwm: P,
        pub adc: A,
        /// The duty in 1/65536 of the maximum.
        integral: i32,
        pub last: u16,
    }

    impl<P: SetDutyCycle, A: AnalogInput> Regulator<P, A> {
        pub fn new(pwm: P, adc: A) -> Self {
            Regulator {
                pwm,
                adc,
                integral: 0,
                last: 0,
            }
        }

        pub fn step(&mut self) {
            self.last = self.adc.read();
            let error = i32::from(TARGET) - i32::from(self.last);
            self.integral = (self.integral + error * 40).clamp(0, 65535);
            let max = u32::from(self.pwm.max_duty_cycle());
            let duty = (self.integral as u32 * max / 65535) as u16;
            self.pwm.set_duty_cycle(duty).unwrap();
        }
    }
}

use firmware::{AnalogInput, Regulator};

impl AnalogInput for AdcPin {
    fn read(&mut self) -> u16 {
        AdcChannel::read(self)
    }
}

type Regulated = Regulator<PwmChannel, AdcPin>;

/// Runs the regulator for `stop` seconds; returns it with the voltage of `OUT` at the end.
fn run(stop: f64) -> Result<(Regulated, f64), Box<dyn std::error::Error>> {
    let board = Board::new();
    let regulator = Regulator::new(board.pwm("pwm", 10e-6, 1000), board.adc("adc", 12, 3.3));
    let mut mcu = Firmware::new(&board, 20e-6, regulator, Regulator::step);
    let sim = bench(stop)?.run_with(DeviceModels::new().with("mcu", &mut mcu))?;
    let table = sim.table("out.txt")?;
    let end = *table.scale().last().ok_or("no time points")?;
    let out = table.interpolate(r#"v("/OUT")"#, end).ok_or("no OUT")?;
    Ok((mcu.into_task(), out))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (regulator, out) = run(3e-3)?;
    println!("OUT: {out:.3} V, last code: {}", regulator.last);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `OUT` settles at 1 V, and the code read reaches the target.
    #[test]
    fn test_regulates_to_target() {
        let (regulator, out) = run(3e-3).unwrap();
        assert!((out - 1.0).abs() < 0.1, "{out}");
        let code = i32::from(regulator.last);
        assert!((code - i32::from(firmware::TARGET)).abs() < 60, "{code}");
    }
}
