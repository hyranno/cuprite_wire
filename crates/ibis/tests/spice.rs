//! Simulates the subcircuits with ngspice (`ngspice` on `PATH`, `ngspice_con` on Windows).

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use cuprite_wire_ibis::{
    Corner, IbisFile, parse,
    spice::{Drive, Parasitics, Subcircuit, Weights, subcircuit},
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
fn test_driver_reference_not_ground() {
    // All the voltages of the model, the weights included, are relative to the port `ref`.
    let dut = out_3s();
    let values = run(&format!(
        "driver
{}X1 pin r ku kd dut
Vr r 0 1
Vku ku r 1
Vkd kd r 0
Rload pin r 50
.control
op
let high = v(pin) - v(r)
print high
.endc
.end
",
        dut.text
    ));
    assert_close(value(&values, "high"), 3.3 * 50.0 / 132.5, 1e-4);
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
    assert_eq!(dut.ports, ["pin", "ref"]);
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
fn test_terminator() {
    // A Thevenin termination (100 ohms to 3.3 V and to 1 V) with an AC termination
    // (50 ohms and 1 nF to 1 V).
    let ibs = parse(
        "\
[IBIS Ver] 5.0
[File Name] term.ibs
[Model] term
Model_type Terminator
C_comp 1p NA NA
[Voltage Range] 3.3 3.0 3.6
[GND Clamp Reference] 1.0 NA NA
[Rgnd] 100 NA NA
[Rpower] 100 NA NA
[Rac] 50 NA NA
[Cac] 1n NA NA
[END]
",
    )
    .unwrap();
    let dut = subcircuit("dut", &ibs.models[0], &Parasitics::NONE, Corner::Typ).unwrap();
    assert_eq!(dut.ports, ["pin", "ref"]);
    assert!(!dut.is_driver());
    // DC: the pin floats to the middle of the references, 2.15 V. AC at 1 GHz: the two 100 ohms,
    // 50 ohms and 1 nF, and C_comp in parallel.
    let values = run(&format!(
        "terminator
{}X1 pin 0 dut
Rs src pin 1g
Vs src 0 0
Ip 0 pin 0 AC 1
.control
op
let open = v(pin)
print open
ac lin 1 1g 1g
let z = mag(v(pin))
print z
.endc
.end
",
        dut.text
    ));
    assert_close(value(&values, "open"), 2.15, 1e-3);
    let w = 2.0 * std::f64::consts::PI * 1e9;
    // The admittance of 50 ohms and 1 nF in series: 1 / (50 - j / (w C)).
    let (re, im) = (50.0, -1.0 / (w * 1e-9));
    let norm = re * re + im * im;
    let (g, b) = (2.0 / 100.0 + re / norm, -im / norm + w * 1e-12);
    assert_close(value(&values, "z"), 1.0 / g.hypot(b), 1e-3);
}

#[test]
fn test_driver_transient() {
    let dut = out_3s();
    // The [Rising Waveform] has L_fixture: the edges come from [Ramp].
    assert_eq!(
        dut.notes,
        ["[Rising Waveform] 1 is not used: L_fixture 0.000000001 not supported"]
    );
    assert!(dut.text.contains("\n* [Rising Waveform] 1 is not used"));
    let edges = dut.edges.unwrap();
    // dV/dt_r 1.98/0.5n, dV/dt_f 1.98/0.4n.
    assert_close(edges.rising.duration(), 0.5e-9 / 0.6, 1e-18);
    assert_close(edges.falling.duration(), 0.4e-9 / 0.6, 1e-18);
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
    assert!(
        rise > 1e-9 && rise < 1e-9 + edges.rising.duration(),
        "{rise}"
    );
}

/// The value of a piecewise linear waveform at `t` (the last value after the end).
fn at(points: &[(f64, f64)], t: f64) -> f64 {
    let t = t.min(points.last().unwrap().0);
    let i = points
        .partition_point(|p| p.0 <= t)
        .clamp(1, points.len() - 1);
    let ((t0, v0), (t1, v1)) = (points[i - 1], points[i]);
    v0 + (v1 - v0) * (t - t0) / (t1 - t0)
}

/// Simulates the driver into fixtures of 50 ohms to ground and to `supply`, each edge starting
/// `delay` after the start, and returns them as `[Rising Waveform]`s and `[Falling Waveform]`s
/// (the steps of `tran`, the arguments of `.tran`).
fn record_waveforms(dut: &Subcircuit, supply: f64, delay: f64, tran: &str) -> String {
    let edges = dut.edges.as_ref().unwrap();
    let rising = edges.weights(Drive::Low, &[(delay, Drive::High)]);
    let falling = edges.weights(Drive::High, &[(delay, Drive::Low)]);
    let mut deck = format!("fixtures\n{}", dut.text);
    for (edge, weights) in [("r", &rising), ("f", &falling)] {
        deck += &pwl(&format!("Vku_{edge}"), &format!("ku_{edge}"), &weights.ku);
        deck += &pwl(&format!("Vkd_{edge}"), &format!("kd_{edge}"), &weights.kd);
        for (fixture, v) in [("0", 0.0), ("1", supply)] {
            deck += &format!(
                "X{edge}{fixture} {edge}{fixture} 0 ku_{edge} kd_{edge} {}\n\
                 R{edge}{fixture} {edge}{fixture} v{edge}{fixture} 50\n\
                 V{edge}{fixture} v{edge}{fixture} 0 {v}\n",
                dut.name
            );
        }
    }
    deck += &format!(
        "\
.tran {tran}
.control
set wr_singlescale
run
linearize v(r0) v(r1) v(f0) v(f1)
wrdata waves.txt v(r0) v(r1) v(f0) v(f1)
.endc
.end
"
    );
    let dir = TempDir::new();
    run_in(&dir.0, &deck);
    let rows: Vec<Vec<f64>> = std::fs::read_to_string(dir.0.join("waves.txt"))
        .unwrap()
        .lines()
        .map(|l| l.split_whitespace().map(|n| n.parse().unwrap()).collect())
        .collect();
    assert!(rows.len() > 50, "{}", rows.len());
    let mut text = String::new();
    for (keyword, columns) in [("Rising", [1, 2]), ("Falling", [3, 4])] {
        for (column, v_fixture) in columns.into_iter().zip([0.0, supply]) {
            text += &format!("[{keyword} Waveform]\nR_fixture = 50\nV_fixture = {v_fixture}\n");
            for row in &rows {
                text += &format!("{:e} {:e} NA NA\n", row[0], row[column]);
            }
        }
    }
    text
}

/// Records the waveforms of `out_3s` (linear edges from [Ramp], `C_comp` 3 pF) into fixtures of
/// 50 ohms to ground and to 3.3 V, writes them into a copy of the model as waveforms, and checks
/// that the weights solved from them drive a different load as the original does.
#[test]
fn test_waveforms() {
    let original = out_3s();
    let edges = original.edges.clone().unwrap();
    // Each edge 0.5 ns after the start of its waveform.
    let waveforms = record_waveforms(&original, 3.3, 0.5e-9, "50p 3n 0 5p");

    // The model with the waveforms.
    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/buffers.ibs"
    ))
    .unwrap();
    let start = fixture.find("[Model]         out_3s").unwrap();
    let end = fixture.find("[Rising Waveform]").unwrap();
    let mut ibs = format!(
        "[IBIS Ver] 5.0\n[File Name] w.ibs\n{}{waveforms}",
        &fixture[start..end]
    );
    ibs += "[END]\n";
    let ibs = parse(&ibs).unwrap();
    let solved = subcircuit("solved", &ibs.models[0], &Parasitics::NONE, Corner::Typ).unwrap();
    assert_eq!(solved.notes, Vec::<String>::new());
    let solved_edges = solved.edges.clone().unwrap();

    // The solved weights are the original ones.
    let rising = edges.weights(Drive::Low, &[(0.5e-9, Drive::High)]);
    let falling = edges.weights(Drive::High, &[(0.5e-9, Drive::Low)]);
    for i in 0..=30 {
        let t = i as f64 * 0.1e-9;
        for (ours, theirs) in [
            (&solved_edges.rising.ku, &rising.ku),
            (&solved_edges.rising.kd, &rising.kd),
            (&solved_edges.falling.ku, &falling.ku),
            (&solved_edges.falling.kd, &falling.kd),
        ] {
            let (a, b) = (at(ours, t), at(theirs, t));
            assert!(
                (a - b).abs() < 0.03,
                "at {t}: {a} != {b}\n{ours:?}\n{theirs:?}"
            );
        }
    }

    // Both drive another load alike, the original 0.5 ns later: the solved edges keep the delay
    // of the waveforms.
    let ours = solved_edges.weights(Drive::Low, &[(1e-9, Drive::High), (4e-9, Drive::Low)]);
    let theirs = edges.weights(Drive::Low, &[(1.5e-9, Drive::High), (4.5e-9, Drive::Low)]);
    let mut deck = format!("load\n{}{}", original.text, solved.text);
    deck += &pwl("Vku1", "ku1", &theirs.ku);
    deck += &pwl("Vkd1", "kd1", &theirs.kd);
    deck += &pwl("Vku2", "ku2", &ours.ku);
    deck += &pwl("Vkd2", "kd2", &ours.kd);
    deck += "\
X1 a 0 ku1 kd1 dut
Ra a mid 100
Ca a 0 5p
X2 b 0 ku2 kd2 solved
Rb b mid 100
Cb b 0 5p
Vmid mid 0 1.65
.tran 10p 8n
.control
run
let diff = abs(v(a) - v(b))
meas tran max_diff max diff
meas tran high find v(b) at=3.5n
.endc
.end
";
    let values = run(&deck);
    assert!(value(&values, "high") > 2.5, "{values:?}");
    assert!(value(&values, "max_diff") < 0.01, "{values:?}");
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
    assert!(text.contains("\nRPKG pin pkg 1e-1\nLPKG pkg die 6e-9\nCPKG pin ref 5e-13\n"));
    assert!(text.contains("\nCCOMP die ref 3e-12\n"));
    assert!(text.contains("\nBPD die ref I=v(i_pd,ref)*v(kd,ref)\n"));
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
        error("Model_type Series"),
        "model `m`: the model type `Series` is not supported"
    );
    assert_eq!(
        error("Model_type Terminator\n[Rac] 50 NA NA"),
        "model `m`: [Rac] and [Cac] go together"
    );
    assert_eq!(
        error("Model_type Terminator\n[Rpower] 50 NA NA"),
        "model `m`: [Rpower] needs [POWER Clamp Reference] or [Voltage Range]"
    );
    assert_eq!(
        error(&format!("Model_type Output\n[Pulldown]{table}")),
        "model `m`: a `Output` model needs [Pullup]"
    );
    assert_eq!(
        error(&format!("Model_type Open_drain\n[Pulldown]{table}")),
        "model `m`: a `Open_drain` model needs [Rising Waveform] or [Ramp]"
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
    assert_eq!(dut.ports, ["pin", "ref", "ku", "kd"]);
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

const KICAD_DEMO: &str = "/usr/share/kicad/demos/simulation/ibis";

/// The conversion of KiCad of the `RECTDRIVER` U4 of the IBIS demo (pin 4, model `Output`,
/// 100 ns high and 100 ns low from 0), with the demo's `ibis_v1_1.ibs` replaced by `ibs`.
fn kibis_rectdriver(dir: &Path, ibs: &str) -> String {
    let demo = Path::new(KICAD_DEMO);
    for file in ["ibis.kicad_sch", "ibis.kicad_pro"] {
        std::fs::copy(demo.join(file), dir.join(file)).unwrap();
    }
    std::fs::write(dir.join("ibis_v1_1.ibs"), ibs).unwrap();
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
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .current_dir(dir)
        .output()
        .unwrap()
        .status;
    assert!(status.success());
    let cache = std::fs::read_dir(dir.join("cache/kicad"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("ibis/U4.cache");
    // KIBIS ends the subcircuit with `.ENDS DRIVER`, which ngspice accepts.
    std::fs::read_to_string(cache).unwrap()
}

/// Our driver of the model `Output` of `ibs` on pin 4, with the weights of the `RECTDRIVER` U4,
/// and the weights.
fn rectdriver(ibs: &str) -> (Subcircuit, Weights) {
    let ibs = parse(ibs).unwrap();
    let component = ibs.component("Virtual").unwrap();
    let parasitics = Parasitics::of_pin(component, component.pin("4").unwrap(), Corner::Typ);
    let dut = subcircuit(
        "dut",
        ibs.model("Output").unwrap(),
        &parasitics,
        Corner::Typ,
    )
    .unwrap();
    let events: Vec<_> = (0..50)
        .flat_map(|i| {
            let t = i as f64 * 200e-9;
            [(t, Drive::High), (t + 100e-9, Drive::Low)]
        })
        .collect();
    let weights = dut.edges.as_ref().unwrap().weights(Drive::Low, &events);
    (dut, weights)
}

/// Checks that our driver and that of KIBIS give the same voltage within `tolerance` on a pin
/// pulled to 5 V and ground by 10 kohm each.
fn compare_with_kibis(
    dir: &Path,
    kibis: &str,
    dut: &Subcircuit,
    weights: &Weights,
    tolerance: f64,
) {
    let mut deck = format!("kibis\n{kibis}\n{}{}", dut.text, weight_sources(weights));
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
    let values = run_in(dir, &deck);
    assert!(value(&values, "high") > 4.9);
    assert!(value(&values, "low") < 0.1);
    assert!(value(&values, "max_diff") < tolerance, "{values:?}");
}

/// Compares a driver with the conversion of KiCad, the `RECTDRIVER` U4 of the IBIS demo (each pin
/// pulled to 5 V and ground by 10 kohm).
#[test]
#[ignore = "needs kicad-cli and the KiCad demos in /usr/share/kicad"]
fn test_kicad_rectdriver() {
    let dir = TempDir::new();
    let ibs = std::fs::read_to_string(Path::new(KICAD_DEMO).join("ibis_v1_1.ibs")).unwrap();
    let kibis = kibis_rectdriver(&dir.0, &ibs);
    let (dut, weights) = rectdriver(&ibs);

    // The same weights as KIBIS (until its last point, before the last falling edge ends).
    for (ours, theirs) in [(&weights.ku, "Vku"), (&weights.kd, "Vkd")] {
        let theirs = kibis_pwl(&kibis, theirs);
        assert!(ours.len() >= theirs.len());
        for (a, b) in ours.iter().zip(&theirs) {
            assert_close(a.0, b.0, 1e-12);
            assert_close(a.1, b.1, 1e-9);
        }
    }
    compare_with_kibis(&dir.0, &kibis, &dut, &weights, 0.01);
}

/// Compares the weights solved from waveforms with those of KiCad: the model `Output` of the demo
/// with the waveforms that its `[Ramp]` gives into fixtures, 5 ns after the start.
#[test]
#[ignore = "needs kicad-cli and the KiCad demos in /usr/share/kicad"]
fn test_kicad_rectdriver_waveforms() {
    let demo = std::fs::read_to_string(Path::new(KICAD_DEMO).join("ibis_v1_1.ibs")).unwrap();
    // The waveforms of the die, without the package.
    let original = subcircuit(
        "original",
        parse(&demo).unwrap().model("Output").unwrap(),
        &Parasitics::NONE,
        Corner::Typ,
    )
    .unwrap();
    let waveforms = record_waveforms(&original, 5.0, 5e-9, "1n 100n 0 0.2n");
    // Waveforms need version 2.1.
    let ibs = demo
        .replacen("[IBIS Ver]      1.1", "[IBIS Ver]      2.1", 1)
        .replacen("\n[END]", &format!("\n{waveforms}[END]"), 1);
    let dir = TempDir::new();
    let kibis = kibis_rectdriver(&dir.0, &ibs);
    let (dut, weights) = rectdriver(&ibs);
    assert_eq!(dut.notes, Vec::<String>::new());

    // The weights of the first period, rising from 5 ns and falling from 105 ns.
    for (ours, theirs) in [(&weights.ku, "Vku"), (&weights.kd, "Vkd")] {
        let theirs = kibis_pwl(&kibis, theirs);
        for i in 0..200 {
            let t = i as f64 * 1e-9;
            let (a, b) = (at(ours, t), at(&theirs, t));
            assert!((a - b).abs() < 0.01, "at {t}: {a} != {b}");
        }
    }
    // The edges differ by a fraction of the time step of the waveforms (1 ns, about 100 mV on the
    // edges of 5 V in 50 ns).
    compare_with_kibis(&dir.0, &kibis, &dut, &weights, 0.03);
}
