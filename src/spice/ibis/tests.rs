use cuprite_wire_ibis::{Corner, IbisFile, parse, spice::Parasitics};

use super::*;
use crate::{
    circuit::CircuitBuilder,
    naming::NetLabel,
    spice::{Simulator, Table, tests::on_each_simulator},
    test_parts::deck_of,
};

/// Linear tables of 82.5 ohms (40 mA at 3.3 V), clamps beyond the supply, and edges of 1 ns
/// (dt / 0.6) over the 20-80 % of the swing.
const IBS: &str = "\
[IBIS Ver] 5.0
[File Name] test.ibs
[Component] Chip
[Manufacturer] Nobody
[Package]
R_pkg 0.1 NA NA
L_pkg 2n NA NA
C_pkg 0.5p NA NA
[Pin] signal_name model_name
1 IO io
2 IN in
3 OD od
4 VCC POWER
5 SEL sel
7 OUT out
[Model Selector] sel
io I/O
in Input
[Model] io
Model_type I/O
Vinl = 0.8
Vinh = 2.0
C_comp 2p NA NA
[Voltage Range] 3.3 NA NA
[Pulldown]
-3.3 -40m NA NA
6.6 80m NA NA
[Pullup]
-3.3 40m NA NA
6.6 -80m NA NA
[GND Clamp]
-3.3 -100m NA NA
-0.6 0 NA NA
6.6 0 NA NA
[POWER Clamp]
-3.3 100m NA NA
-0.6 0 NA NA
6.6 0 NA NA
[Ramp]
dV/dt_r 1.98/0.6n NA NA
dV/dt_f 1.98/0.6n NA NA
[Model] in
Model_type Input
Vinl = 1.0
Vinh = 2.3
C_comp 1p NA NA
[Voltage Range] 3.3 NA NA
[GND Clamp]
-3.3 -100m NA NA
-0.6 0 NA NA
6.6 0 NA NA
[Model] od
Model_type Open_drain
C_comp 2p NA NA
[Pulldown]
-3.3 -40m NA NA
6.6 80m NA NA
[Ramp]
dV/dt_r 1.98/0.6n NA NA
dV/dt_f 1.98/0.6n NA NA
[Model] out
Model_type Output
[Voltage Range] 3.3 NA NA
[Pulldown]
-3.3 -40m NA NA
6.6 80m NA NA
[Pullup]
-3.3 40m NA NA
6.6 -80m NA NA
[Ramp]
dV/dt_r 1.98/0.6n NA NA
dV/dt_f 1.98/0.6n NA NA
[END]
";

fn ibs() -> IbisFile {
    parse(IBS).unwrap()
}

fn buffer(pin: &str) -> IbisBuffer {
    IbisBuffer::of_pin(&ibs(), "Chip", pin, Corner::Typ).unwrap()
}

#[test]
fn test_of_pin_errors() {
    let error = |component, pin| {
        IbisBuffer::of_pin(&ibs(), component, pin, Corner::Typ)
            .unwrap_err()
            .to_string()
    };
    assert_eq!(error("Other", "1"), "no [Component] `Other`");
    assert_eq!(error("Chip", "9"), "component `Chip` has no pin `9`");
    assert_eq!(
        error("Chip", "4"),
        "pin `4` has no model: it is a POWER pin"
    );
    let model_error = |pin, model| {
        IbisBuffer::of_pin_model(&ibs(), "Chip", pin, model, Corner::Typ)
            .unwrap_err()
            .to_string()
    };
    assert_eq!(
        model_error("5", "od"),
        "pin `5` has no model: `od` is not a model of `sel`"
    );
    assert_eq!(
        model_error("1", "in"),
        "pin `1` has no model: `in` is not a model of `io`"
    );
}

#[test]
fn test_model_selector() {
    // The first model of the selector is the default.
    let default = IbisBuffer::of_pin(&ibs(), "Chip", "5", Corner::Typ).unwrap();
    assert_eq!(default.model_name(), "io");
    let chosen = IbisBuffer::of_pin_model(&ibs(), "Chip", "5", "in", Corner::Typ).unwrap();
    assert_eq!(chosen.model_name(), "in");
    assert!(!chosen.is_driver());
    // A pin naming a model takes the model's own name.
    let io = IbisBuffer::of_pin_model(&ibs(), "Chip", "1", "io", Corner::Typ).unwrap();
    assert_eq!(io, buffer("1"));
}

#[test]
fn test_buffer() {
    let io = buffer("1");
    assert_eq!(io.model_name(), "io");
    assert!(io.is_driver());
    assert_eq!(io.edges().unwrap().rising.duration(), 1e-9);
    assert_eq!(io.notes(), [] as [String; 0]);
    let input = io.input().unwrap();
    assert_eq!((input.threshold_low, input.threshold_high), (0.8, 2.0));
    assert_eq!(input.capacitance, 2e-12);
    assert!(io.name.starts_with("cuprite_ibis_model_"), "{}", io.name);
    assert!(
        io.text
            .contains(&format!("\n.subckt {} pin ref ku kd\n", io.name)),
        "{}",
        io.text
    );
    // The package is part of the definition.
    let ibs = ibs();
    let bare = IbisBuffer::new(ibs.model("io").unwrap(), &Parasitics::NONE, Corner::Typ).unwrap();
    assert_ne!(bare.name, io.name);
    assert_eq!(buffer("1").name, io.name);

    let od = buffer("3");
    assert!(od.input().is_none());
    assert!(od.can_drive(Level::Low) && od.can_drive(Level::HiZ));
    assert!(!od.can_drive(Level::High));
    let out = buffer("7");
    assert!(!out.can_drive(Level::HiZ));
    assert!(!buffer("2").can_drive(Level::Low));
}

#[test]
#[should_panic(expected = "the IBIS model `out` (Output) cannot drive HiZ")]
fn test_driver_level_not_driven() {
    let _ = IbisDriver::new(
        buffer("7"),
        DigitalSequence::new(Level::Low).at(1e-9, Level::HiZ),
    );
}

#[test]
#[should_panic(expected = "the IBIS model `in` (Input) does not drive")]
fn test_driver_of_input() {
    let _ = IbisDriver::new(buffer("2"), DigitalSequence::new(Level::Low));
}

#[test]
fn test_driver_subcircuit() {
    let io = buffer("1");
    let driver = IbisDriver::new(
        io.clone(),
        DigitalSequence::new(Level::Low).at(1e-9, Level::High),
    );
    let (name, text) = driver.subcircuit();
    assert!(name.starts_with("cuprite_ibis_"), "{name}");
    assert_eq!(
        text,
        format!(
            ".subckt {name} out ref\n\
             VKU ku ref PWL( 0 0 1e-9 0 2e-9 1 )\n\
             VKD kd ref PWL( 0 1 1e-9 1 2e-9 0 )\n\
             XB out ref ku kd {}\n\
             .ends {name}\n",
            io.name
        )
    );
    let Some(SpicePartInfo::Elements { elements, .. }) = driver.spice() else {
        panic!()
    };
    assert_eq!(elements[0].models.len(), 2);
    // An I/O receiver turns the driver off; an input is its subcircuit.
    let Some(SpicePartInfo::Elements { elements, .. }) = IbisReceiver::new(io.clone()).spice()
    else {
        panic!()
    };
    let ModelSource::Inline { text, .. } = &elements[0].models[1] else {
        panic!()
    };
    assert!(
        text.contains(&format!("\nXB in ref ref ref {}\n", io.name)),
        "{text}"
    );
    let input = buffer("2");
    let Some(SpicePartInfo::Elements { elements, .. }) = IbisReceiver::new(input.clone()).spice()
    else {
        panic!()
    };
    assert_eq!(elements[0].args, input.name);
    assert_eq!(elements[0].models.len(), 1);
}

on_each_simulator!(simulate_ibis_line, simulate_ibis_open_drain);

/// The voltage of the line in the table of [`simulate`].
const LINE: &str = r#"v("/LINE")"#;

/// The driver and the receiver on the line `/LINE`, with a 1 kohm pull-up if `pull_up`, simulated for
/// `stop` seconds; the table has the voltage of the line ([`LINE`]).
fn simulate(
    simulator: &dyn Simulator,
    driver: IbisDriver,
    receiver: IbisReceiver,
    pull_up: bool,
    stop: f64,
) -> Table {
    let mut circuit = CircuitBuilder::new();
    let driver = circuit.instantiate_part(driver);
    let receiver = circuit.instantiate_part(receiver);
    circuit.connect(driver[IbisDriver::OUT], receiver[IbisReceiver::IN]);
    circuit.label(driver[IbisDriver::OUT], NetLabel::local("LINE"));
    circuit.label(driver[IbisDriver::GND], NetLabel::global("GND"));
    circuit.label(receiver[IbisReceiver::GND], NetLabel::global("GND"));
    let mut control = Vec::new();
    if pull_up {
        // 1 kohm to 3.3 V, written as its Norton equivalent.
        let load = circuit.instantiate_part(crate::spice::digital::DigitalLoad::new(
            InputCharacteristics::cmos(3.3)
                .with_capacitance(0.0)
                .with_pull(crate::spice::digital::Pull::up(3.3, 1e3)),
        ));
        circuit.connect(driver[IbisDriver::OUT], load["in"]);
        circuit.label(load["gnd"], NetLabel::global("GND"));
    }
    control.push(format!("tran 10p {stop:e}"));
    control.push(format!("wrdata line.txt {LINE}"));
    let deck = deck_of(&circuit).control(control);
    simulator.run(&deck).unwrap().table("line.txt").unwrap()
}

/// The I/O buffer drives the input buffer, which reads the levels back with its thresholds.
fn simulate_ibis_line(simulator: &dyn Simulator) {
    let rx = buffer("2");
    let sent = DigitalSequence::new(Level::Low)
        .at(5e-9, Level::High)
        .at(25e-9, Level::Low)
        .at(45e-9, Level::HiZ);
    let table = simulate(
        simulator,
        IbisDriver::new(buffer("1"), sent),
        IbisReceiver::new(rx.clone()),
        false,
        60e-9,
    );
    let at = |t| table.interpolate(LINE, t).unwrap();
    // Unloaded, the line settles at the rails.
    assert!(at(4e-9).abs() < 1e-3, "{}", at(4e-9));
    assert!((at(20e-9) - 3.3).abs() < 1e-3, "{}", at(20e-9));
    assert!(at(40e-9).abs() < 1e-3, "{}", at(40e-9));
    let read = rx
        .input()
        .unwrap()
        .decode(table.scale(), table.column(LINE).unwrap());
    assert_eq!(read.initial(), Level::Low);
    let [(rise, Level::High), (fall, Level::Low)] = read.events() else {
        panic!("{read:?}");
    };
    // The edges take 1 ns, delayed by the package and the capacitances.
    assert!(*rise > 5.5e-9 && *rise < 7e-9, "{rise}");
    assert!(*fall > 25.5e-9 && *fall < 27e-9, "{fall}");
}

/// The open drain buffer against a pull-up into an I/O receiver: low through 82.5 ohms, high
/// once released.
fn simulate_ibis_open_drain(simulator: &dyn Simulator) {
    let sent = DigitalSequence::new(Level::HiZ)
        .at(10e-9, Level::Low)
        .at(30e-9, Level::HiZ);
    let table = simulate(
        simulator,
        IbisDriver::new(buffer("3"), sent),
        IbisReceiver::new(buffer("1")),
        true,
        60e-9,
    );
    let at = |t| table.interpolate(LINE, t).unwrap();
    assert!((at(9e-9) - 3.3).abs() < 1e-3, "{}", at(9e-9));
    let low = 3.3 * 82.5 / 1082.5;
    assert!((at(28e-9) - low).abs() < 1e-3, "{}", at(28e-9));
    assert!((at(59e-9) - 3.3).abs() < 0.05, "{}", at(59e-9));
}
