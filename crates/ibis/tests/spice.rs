//! Simulates the subcircuits with ngspice (`ngspice` on `PATH`, `ngspice_con` on Windows).

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use cuprite_wire_ibis::{
    Corner, IbisFile, parse,
    spice::{Drive, EdgeTimes, Parasitics, Subcircuit, Weights, subcircuit},
};

fn fixture() -> IbisFile {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/buffers.ibs");
    parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A new empty directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "cuprite_wire_ibis-test-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs the deck in `dir` and returns the values it prints as `name = value` (`print`, `meas`).
fn run_in(dir: &Path, deck: &str) -> Vec<(String, f64)> {
    std::fs::write(dir.join("deck.cir"), deck).unwrap();
    // Keeps the user's ~/.spiceinit out.
    std::fs::write(dir.join(".spiceinit"), "unset ngbehavior\n").unwrap();
    let program = if cfg!(windows) {
        "ngspice_con"
    } else {
        "ngspice"
    };
    let output = Command::new(program)
        .args(["-b", "deck.cir"])
        .current_dir(dir)
        .output()
        .unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success()
            && !log.lines().any(|l| {
                let l = l.trim_start().to_ascii_lowercase();
                l.starts_with("error") || l.ends_with("simulation(s) aborted")
            }),
        "{deck}\n{log}"
    );
    log.lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(" = ")?;
            let value = value.split_whitespace().next()?.parse().ok()?;
            Some((name.trim().to_string(), value))
        })
        .collect()
}

fn run(deck: &str) -> Vec<(String, f64)> {
    run_in(&TempDir::new().0, deck)
}

fn value(values: &[(String, f64)], name: &str) -> f64 {
    values
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no {name} in {values:?}"))
        .1
}

fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} != {expected} (+/- {tolerance})"
    );
}

/// A PWL source of a weight waveform.
fn pwl(name: &str, node: &str, points: &[(f64, f64)]) -> String {
    let points: Vec<_> = points.iter().map(|(t, v)| format!("{t:e} {v:e}")).collect();
    format!("{name} {node} 0 PWL( {} )\n", points.join(" "))
}

fn weight_sources(weights: &Weights) -> String {
    pwl("Vku", "ku", &weights.ku) + &pwl("Vkd", "kd", &weights.kd)
}

fn out_3s() -> Subcircuit {
    let ibs = fixture();
    subcircuit(
        "dut",
        ibs.model("out_3s").unwrap(),
        &Parasitics::NONE,
        Corner::Typ,
    )
    .unwrap()
}

#[test]
fn test_driver_dc() {
    // The tables of out_3s are 82.5 ohms; the supply is 3.3 V.
    let dut = out_3s();
    let values = run(&format!(
        "driver
{}X1 pin 0 ku kd dut
Vku ku 0 1
Vkd kd 0 0
Rload pin load 50
Vload load 0 0
.control
op
print v(pin)
alter Vku 0
alter Vkd 1
alter Vload 3.3
op
let low = v(pin)
print low
alter Vkd 0
op
let hiz = v(pin)
print hiz
.endc
.end
",
        dut.text
    ));
    assert_close(value(&values, "v(pin)"), 3.3 * 50.0 / 132.5, 1e-4);
    assert_close(value(&values, "low"), 3.3 * 82.5 / 132.5, 1e-4);
    assert_close(value(&values, "hiz"), 3.3, 1e-6);
}

#[test]
fn test_receiver_clamps() {
    // The tables span the voltages of the test, as those of real models do: beyond them, `pwl`
    // extrapolates the end segments.
    let ibs = parse(
        "\
[IBIS Ver] 5.0
[File Name] rx.ibs
[Model] rx
Model_type Input
C_comp 1p NA NA
[Voltage Range] 3.3 3.0 3.6
[GND Clamp]
-3.3 -100m -80m NA
-0.6 0 0 NA
6.6 0 0 NA
[POWER Clamp]
-3.3 100m NA NA
-0.6 0 NA NA
6.6 0 NA NA
[END]
",
    )
    .unwrap();
    let dut = subcircuit("dut", &ibs.models[0], &Parasitics::NONE, Corner::Min).unwrap();
    assert_eq!(dut.ports, ["pin", "gnd"]);
    assert!(!dut.is_driver());
    // -i(Vpin) is the current into the pin. The reference of [POWER Clamp] is the min of
    // [Voltage Range], 3.0 V, and the min of its currents is NA.
    let values = run(&format!(
        "receiver
{}X1 pin 0 dut
Vpin pin 0 -3.3
.control
op
let gnd_clamp = -i(Vpin)
print gnd_clamp
alter Vpin 6.3
op
let power_clamp = -i(Vpin)
print power_clamp
alter Vpin 1
op
let inside = -i(Vpin)
print inside
.endc
.end
",
        dut.text
    ));
    assert_close(value(&values, "gnd_clamp"), -80e-3, 1e-6);
    assert_close(value(&values, "power_clamp"), 100e-3, 1e-6);
    assert_close(value(&values, "inside"), 0.0, 1e-9);
}

#[test]
fn test_driver_transient() {
    let dut = out_3s();
    let edges = dut.edge_times.unwrap();
    // dV/dt_r 1.98/0.5n, dV/dt_f 1.98/0.4n.
    assert_close(edges.rise, 0.5e-9 / 0.6, 1e-18);
    assert_close(edges.fall, 0.4e-9 / 0.6, 1e-18);
    let weights = edges.weights(
        Drive::HiZ,
        &[(1e-9, Drive::High), (5e-9, Drive::Low), (9e-9, Drive::HiZ)],
    );
    let values = run(&format!(
        "transient
{}{}X1 pin 0 ku kd dut
Rload pin mid 50
Vmid mid 0 1.65
.tran 10p 12n
.control
run
meas tran hiz find v(pin) at=0.9n
meas tran high find v(pin) at=4n
meas tran low find v(pin) at=8n
meas tran hiz2 find v(pin) at=12n
meas tran rise when v(pin)=2 rise=1
.endc
.end
",
        dut.text,
        weight_sources(&weights)
    ));
    // 82.5 ohms to 3.3 V or ground against 50 ohms to 1.65 V.
    assert_close(value(&values, "hiz"), 1.65, 1e-3);
    assert_close(value(&values, "high"), 1.65 + 1.65 * 50.0 / 132.5, 1e-3);
    assert_close(value(&values, "low"), 1.65 * 82.5 / 132.5, 1e-3);
    assert_close(value(&values, "hiz2"), 1.65, 1e-3);
    // Within the edge.
    let rise = value(&values, "rise");
    assert!(rise > 1e-9 && rise < 1e-9 + edges.rise, "{rise}");
}

#[test]
fn test_package() {
    let ibs = fixture();
    let component = ibs.component("Buffer8").unwrap();
    // R_pin, L_pin and C_pin of the pin.
    let p = Parasitics::of_pin(component, component.pin("2").unwrap(), Corner::Typ);
    assert_close(p.r.unwrap(), 0.1, 1e-15);
    assert_close(p.l.unwrap(), 3.5e-9, 1e-20);
    assert_close(p.c.unwrap(), 0.6e-12, 1e-24);
    // L_pkg of the component for the NA of the pin.
    let p = Parasitics::of_pin(component, component.pin("1").unwrap(), Corner::Max);
    assert_close(p.l.unwrap(), 6e-9, 1e-20);

    let model = ibs.model("out_3s").unwrap();
    let p = Parasitics::new(Some(0.1), Some(6e-9), Some(0.5e-12));
    let text = subcircuit("dut", model, &p, Corner::Typ).unwrap().text;
    assert!(text.contains("\nRPKG pin pkg 1e-1\nLPKG pkg die 6e-9\nCPKG pin gnd 5e-13\n"));
    assert!(text.contains("\nCCOMP die gnd 3e-12\n"));
    assert!(text.contains("\nBPD die gnd I=v(i_pd,gnd)*v(kd,gnd)\n"));
    let text = subcircuit(
        "dut",
        model,
        &Parasitics::new(None, Some(1e-9), None),
        Corner::Typ,
    )
    .unwrap()
    .text;
    assert!(text.contains("\nLPKG pin die 1e-9\n"));
    assert!(!text.contains("CPKG"));
}

#[test]
fn test_errors() {
    let model = |body: &str| {
        let ibs = parse(&format!(
            "[IBIS Ver] 5.0\n[File Name] t.ibs\n[Model] m\n{body}\n[END]\n"
        ))
        .unwrap();
        ibs.models[0].clone()
    };
    let error = |body: &str| {
        subcircuit("x", &model(body), &Parasitics::NONE, Corner::Typ)
            .unwrap_err()
            .to_string()
    };
    let table = "\n0 0 NA NA\n3.3 10m NA NA";
    assert_eq!(
        error("Model_type Terminator"),
        "model `m`: the model type `Terminator` is not supported"
    );
    assert_eq!(
        error(&format!("Model_type Output\n[Pulldown]{table}")),
        "model `m`: a `Output` model needs [Pullup]"
    );
    assert_eq!(
        error(&format!("Model_type Open_drain\n[Pulldown]{table}")),
        "model `m`: a `Open_drain` model needs [Ramp]"
    );
    assert_eq!(
        error("Model_type Input\n[GND Clamp]\n0 0 NA NA"),
        "model `m`: [GND Clamp] needs at least two rows"
    );
    assert_eq!(
        error("Model_type Input\n[GND Clamp]\n0 0 NA NA\n0 1 NA NA"),
        "model `m`: [GND Clamp] repeats the voltage 0"
    );
    assert_eq!(
        error(&format!("Model_type Input\n[POWER Clamp]{table}")),
        "model `m`: [POWER Clamp] needs its reference voltage or [Voltage Range]"
    );
    // An open drain model needs no [Pullup].
    let ramp = "[Ramp]\ndV/dt_r 1/1n NA NA\ndV/dt_f 1/1n NA NA";
    let open_drain = model(&format!("Model_type Open_drain\n[Pulldown]{table}\n{ramp}"));
    let dut = subcircuit("x", &open_drain, &Parasitics::NONE, Corner::Typ).unwrap();
    assert_eq!(dut.ports, ["pin", "gnd", "ku", "kd"]);
    assert!(!dut.text.contains("PU"));
}

/// The numbers of the PWL source `name` of a KIBIS subcircuit, as (time, value) points.
fn kibis_pwl(text: &str, name: &str) -> Vec<(f64, f64)> {
    let line = text
        .lines()
        .find(|l| l.starts_with(&format!("{name} ")))
        .unwrap();
    let inside = &line[line.find('(').unwrap() + 1..line.rfind(')').unwrap()];
    let numbers: Vec<f64> = inside
        .split_whitespace()
        .map(|n| n.parse().unwrap())
        .collect();
    numbers.chunks(2).map(|c| (c[0], c[1])).collect()
}

/// Compares a driver with the conversion of KiCad, the `RECTDRIVER` U4 of the IBIS demo (pin 4,
/// model `Output`, 100 ns high and 100 ns low from 0, each pin pulled to 5 V and ground by
/// 10 kohm).
#[test]
#[ignore = "needs kicad-cli and the KiCad demos in /usr/share/kicad"]
fn test_kicad_rectdriver() {
    let demo = Path::new("/usr/share/kicad/demos/simulation/ibis");
    let dir = TempDir::new();
    for file in ["ibis.kicad_sch", "ibis.kicad_pro", "ibis_v1_1.ibs"] {
        std::fs::copy(demo.join(file), dir.0.join(file)).unwrap();
    }
    // kicad-cli writes the conversion into its cache while exporting the netlist.
    let status = Command::new("kicad-cli")
        .args([
            "sch",
            "export",
            "netlist",
            "--format",
            "spice",
            "-o",
            "ibis.cir",
            "ibis.kicad_sch",
        ])
        .env("XDG_CACHE_HOME", dir.0.join("cache"))
        .current_dir(&dir.0)
        .output()
        .unwrap()
        .status;
    assert!(status.success());
    let cache = std::fs::read_dir(dir.0.join("cache/kicad"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("ibis/U4.cache");
    // KIBIS ends the subcircuit with `.ENDS DRIVER`, which ngspice accepts.
    let kibis = std::fs::read_to_string(cache).unwrap();

    let ibs = parse(&std::fs::read_to_string(demo.join("ibis_v1_1.ibs")).unwrap()).unwrap();
    let component = ibs.component("Virtual").unwrap();
    let parasitics = Parasitics::of_pin(component, component.pin("4").unwrap(), Corner::Typ);
    let dut = subcircuit(
        "dut",
        ibs.model("Output").unwrap(),
        &parasitics,
        Corner::Typ,
    )
    .unwrap();
    let edges: EdgeTimes = dut.edge_times.unwrap();
    let events: Vec<_> = (0..50)
        .flat_map(|i| {
            let t = i as f64 * 200e-9;
            [(t, Drive::High), (t + 100e-9, Drive::Low)]
        })
        .collect();
    let weights = edges.weights(Drive::Low, &events);

    // The same weights as KIBIS (until its last point, before the last falling edge ends).
    for (ours, theirs) in [(&weights.ku, "Vku"), (&weights.kd, "Vkd")] {
        let theirs = kibis_pwl(&kibis, theirs);
        assert!(ours.len() >= theirs.len());
        for (a, b) in ours.iter().zip(&theirs) {
            assert_close(a.0, b.0, 1e-12);
            assert_close(a.1, b.1, 1e-9);
        }
    }

    // The same pin voltage.
    let mut deck = format!("kibis\n{kibis}\n{}{}", dut.text, weight_sources(&weights));
    deck += "\
VDD vdd 0 5
XKIBIS 0 kibis U4.Virtual
R1 vdd kibis 10k
R2 kibis 0 10k
XDUT ours 0 ku kd dut
R3 vdd ours 10k
R4 ours 0 10k
.tran 0.1n 600n
.control
run
let diff = abs(v(ours) - v(kibis))
meas tran max_diff max diff
meas tran high find v(ours) at=90n
meas tran low find v(ours) at=190n
.endc
.end
";
    let values = run_in(&dir.0, &deck);
    assert!(value(&values, "high") > 4.9);
    assert!(value(&values, "low") < 0.1);
    assert!(value(&values, "max_diff") < 0.01, "{values:?}");
}
