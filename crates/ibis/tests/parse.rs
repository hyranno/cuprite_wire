//! Reads the hand-written fixture and checks the errors.

use cuprite_wire_ibis::{Corner, Enable, ModelType, PinModel, Polarity, Slope, parse};

fn fixture() -> cuprite_wire_ibis::IbisFile {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/buffers.ibs");
    parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn approx(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= expected.abs() * 1e-9,
        "{actual} != {expected}"
    );
}

#[test]
fn test_header() {
    let h = fixture().header;
    assert_eq!(h.ibis_ver, "5.0");
    assert_eq!(h.comment_char, '!');
    assert_eq!(h.file_name.as_deref(), Some("buffers.ibs"));
    assert_eq!(h.file_rev.as_deref(), Some("1.0"));
    assert_eq!(h.date.as_deref(), Some("2026-11-22"));
    assert_eq!(h.source.as_deref(), Some("Written by hand\nfor the tests."));
    assert_eq!(
        h.notes.as_deref(),
        Some("Two components share the models.\nA keyword of text may repeat.")
    );
    assert_eq!(h.disclaimer.as_deref(), Some("Not a real device."));
    assert_eq!(h.copyright.as_deref(), Some("None."));
}

#[test]
fn test_components() {
    let ibs = fixture();
    let names: Vec<_> = ibs.components.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Buffer8", "Small"]);

    let c = ibs.component("Buffer8").unwrap();
    // [Manufacturer] of [Define Package Model] does not replace it.
    assert_eq!(c.manufacturer.as_deref(), Some("Example Corp."));
    let r = c.package.r_pkg.unwrap();
    assert_eq!((r.typ, r.min, r.max), (0.2, Some(0.1), Some(0.3)));
    approx(c.package.l_pkg.unwrap().get(Corner::Max), 6e-9);
    let cp = c.package.c_pkg.unwrap();
    approx(cp.typ, 1e-12);
    assert_eq!((cp.min, cp.max), (None, None));
    assert_eq!(cp.get(Corner::Min), cp.typ);

    let models: Vec<_> = c.pins.iter().map(|p| (p.name.as_str(), &p.model)).collect();
    assert_eq!(
        models,
        [
            ("1", &PinModel::Named("in_cmos".into())),
            ("2", &PinModel::Named("out_3s".into())),
            ("3", &PinModel::Named("io_sel".into())),
            ("4", &PinModel::Power),
            ("5", &PinModel::Gnd),
            ("6", &PinModel::Nc),
        ]
    );
    let p = c.pin("2").unwrap();
    assert_eq!(p.signal_name, "Y0");
    approx(p.r_pin.unwrap(), 0.1);
    approx(p.l_pin.unwrap(), 3.5e-9);
    approx(p.c_pin.unwrap(), 0.6e-12);
    let p = c.pin("1").unwrap();
    assert_eq!(p.l_pin, None);
    let [diff] = &c.diff_pins[..] else {
        panic!("{:?}", c.diff_pins)
    };
    assert_eq!((diff.pin.as_str(), diff.inv_pin.as_str()), ("1", "2"));
    approx(diff.vdiff.unwrap(), 0.1);
    assert_eq!(diff.tdelay, None);
    assert_eq!(c.diff_pin("2"), Some(diff));
    assert_eq!(c.diff_pin("3"), None);

    let small = ibs.component("Small").unwrap();
    let pin = small.pin("A1").unwrap();
    assert_eq!((pin.r_pin, pin.l_pin, pin.c_pin), (None, None, None));
    let [selector] = &ibs.model_selectors[..] else {
        panic!("{:?}", ibs.model_selectors)
    };
    assert_eq!(selector.name, "io_sel");
    let options: Vec<_> = selector
        .options
        .iter()
        .map(|o| (o.model.as_str(), o.description.as_str()))
        .collect();
    assert_eq!(
        options,
        [("in_cmos", "CMOS input"), ("out_3s", "3-state output")]
    );
    assert_eq!(selector.default_model(), "in_cmos");
    assert_eq!(ibs.model_selector("io_sel"), Some(selector));
}

#[test]
fn test_pin_model() {
    let ibs = fixture();
    let name = |choice| ibs.pin_model("io_sel", choice).map(|m| m.name.as_str());
    assert_eq!(name(None), Ok("in_cmos"));
    assert_eq!(name(Some("out_3s")), Ok("out_3s"));
    assert_eq!(
        name(Some("in_ttl")).unwrap_err().to_string(),
        "`in_ttl` is not a model of `io_sel`"
    );
    assert_eq!(
        ibs.pin_model("out_3s", Some("out_3s")).unwrap().name,
        "out_3s"
    );
    assert_eq!(
        ibs.pin_model("out_3s", Some("in_cmos"))
            .unwrap_err()
            .to_string(),
        "`in_cmos` is not a model of `out_3s`"
    );
    assert_eq!(
        ibs.pin_model("nothing", None).unwrap_err().to_string(),
        "the file has no [Model] or [Model Selector] `nothing`"
    );
}

#[test]
fn test_input_model() {
    let ibs = fixture();
    let names: Vec<_> = ibs.models.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["in_cmos", "out_3s"]);

    let m = ibs.model("in_cmos").unwrap();
    assert_eq!(m.model_type, ModelType::Input);
    assert_eq!(m.polarity, Some(Polarity::NonInverting));
    assert_eq!(m.enable, None);
    assert_eq!((m.vinl, m.vinh), (Some(0.8), Some(2.0)));
    // C_comp takes precedence over C_comp_pullup.
    approx(m.c_comp.unwrap().get(Corner::Min), 1.5e-12);
    // [Model Spec] gives Vinh at the corners; Vinl stays that of the model.
    assert_eq!(m.model_spec.vinh.unwrap().max, Some(2.2));
    assert_eq!(m.model_spec.vinl, None);
    assert_eq!(m.thresholds(Corner::Max), Some((0.8, 2.2)));
    assert_eq!(m.thresholds(Corner::Typ), Some((0.8, 2.1)));
    assert_eq!(m.temperature_range.unwrap().max, Some(0.0));
    assert_eq!(m.voltage_range.unwrap().get(Corner::Max), 3.6);
    assert_eq!(m.pullup_reference, None);
    let gnd = m.gnd_clamp.as_ref().unwrap();
    let rows: Vec<_> = gnd
        .points
        .iter()
        .map(|p| (p.voltage, p.current.typ, p.current.max))
        .collect();
    assert_eq!(
        rows,
        [
            (-3.3, Some(-0.1), Some(-0.12)),
            (-0.6, Some(0.0), Some(0.0)),
            (3.3, Some(0.0), Some(0.0))
        ]
    );
    assert_eq!(m.power_clamp.as_ref().unwrap().points.len(), 2);
    assert_eq!((&m.pullup, &m.pulldown, &m.ramp), (&None, &None, &None));
}

#[test]
fn test_output_model() {
    let ibs = fixture();
    let m = ibs.model("out_3s").unwrap();
    assert_eq!(m.model_type, ModelType::ThreeState);
    assert_eq!(m.polarity, Some(Polarity::Inverting));
    assert_eq!(m.enable, Some(Enable::ActiveLow));
    // The sum of C_comp_pullup and C_comp_pulldown.
    let c_comp = m.c_comp.unwrap();
    approx(c_comp.typ, 3e-12);
    approx(c_comp.get(Corner::Min), 2.5e-12);
    assert_eq!(c_comp.max, None);
    assert_eq!(m.thresholds(Corner::Typ), None);
    assert_eq!(
        (m.vmeas, m.cref, m.rref, m.vref),
        (Some(1.65), Some(0.0), Some(50.0), Some(0.0))
    );
    assert_eq!(m.pullup_reference.unwrap().min, Some(3.0));
    assert_eq!(m.pulldown_reference.unwrap().typ, 0.0);
    assert_eq!(m.power_clamp_reference.unwrap().typ, 3.3);
    assert_eq!(m.gnd_clamp_reference.unwrap().typ, 0.0);
    assert_eq!(m.voltage_range, None);
    assert_eq!(
        m.pulldown.as_ref().unwrap().points[2].current.typ,
        Some(0.04)
    );
    // [Pullup] of the [Submodel] after the model does not replace it.
    assert_eq!(m.pullup.as_ref().unwrap().points.len(), 3);

    let ramp = m.ramp.unwrap();
    let Slope { dv, dt } = ramp.dv_dt_r.get(Corner::Max);
    assert_eq!(dv, 2.16);
    approx(dt, 0.4e-9);
    assert_eq!(ramp.dv_dt_f.min, None);
    assert_eq!(ramp.r_load, Some(50.0));

    assert_eq!(m.falling_waveforms, []);
    let [w] = &m.rising_waveforms[..] else {
        panic!("{:?}", m.rising_waveforms)
    };
    assert_eq!(w.r_fixture, 50.0);
    assert_eq!(w.v_fixture.get(Corner::Min), 3.0);
    assert_eq!(w.v_fixture.get(Corner::Max), 3.6);
    assert_eq!((w.c_fixture, w.r_dut), (Some(0.0), None));
    approx(w.l_fixture.unwrap(), 1e-9);
    assert_eq!(w.points.len(), 2);
    approx(w.points[1].time, 1e-9);
    assert_eq!(w.points[1].voltage.max, None);
    // A column takes the rows with values, the typical column when it has none.
    assert_eq!(w.curve(Corner::Min), [(1e-9, 3.0)]);
    assert_eq!(w.curve(Corner::Max), [(0.0, 0.0), (1e-9, 3.3)]);
}

#[test]
fn test_skipped() {
    let skipped: Vec<_> = fixture()
        .skipped
        .into_iter()
        .map(|s| (s.name, s.line))
        .collect();
    let expected = [
        ("[Package Model]", 26),
        ("[Define Package Model]", 37),
        ("Vmeas", 75),
        ("[Submodel]", 121),
        // The keywords in the scope of [Submodel] are skipped too.
        ("[Pullup]", 123),
    ];
    let expected: Vec<_> = expected.iter().map(|&(n, l)| (n.to_string(), l)).collect();
    assert_eq!(skipped, expected);
}

#[test]
fn test_diff_and_series() {
    let ibs = parse(
        "\
[IBIS Ver] 4.0
[Component] c
[Pin] signal_name model_name
1 P in
2 N in
3 Q+ in
4 Q- in
5 A in
6 B in
[Diff Pin] inv_pin vdiff tdelay_typ tdelay_min tdelay_max
1 2 0.2 1n NA 2n
3 4 NA
[Series Pin Mapping] pin_2 model_name function_table_group
2 1 term
5 6 switch g1
[Model] in
Model_type Input
[Model] term
Model_type Series
[R Series] 100 95 105
[L Series] 1n NA NA
[Rl Series] 0.5 NA NA
[C Series] 1p NA NA
[Rc Series] 2 NA NA
[Model] switch
Model_type Series_switch
[On]
[R Series] 5 NA NA
[Off]
[R Series] 1M NA NA
[Series MOSFET]
Vds = 1.0
0 0 NA NA
[END]
",
    )
    .unwrap();
    let c = ibs.component("c").unwrap();
    let d = c.diff_pin("1").unwrap();
    approx(d.vdiff.unwrap(), 0.2);
    let tdelay = d.tdelay.unwrap();
    approx(tdelay.typ, 1e-9);
    assert_eq!(tdelay.min, None);
    approx(tdelay.get(Corner::Max), 2e-9);
    let d = c.diff_pin("4").unwrap();
    assert_eq!((d.vdiff, d.tdelay), (None, None));

    let m = c.series_pin_mapping("1", "2").unwrap();
    assert_eq!((m.pin1.as_str(), m.model.as_str()), ("2", "term"));
    assert_eq!(m.function_table_group, None);
    let m = c.series_pin_mapping("6", "5").unwrap();
    assert_eq!(m.function_table_group.as_deref(), Some("g1"));
    assert_eq!(c.series_pin_mapping("1", "3"), None);

    let series = ibs.model("term").unwrap().series;
    approx(series.r.unwrap().get(Corner::Min), 95.0);
    approx(series.l.unwrap().typ, 1e-9);
    approx(series.rl.unwrap().typ, 0.5);
    approx(series.c.unwrap().typ, 1e-12);
    assert_eq!(series.lc, None);
    approx(series.rc.unwrap().typ, 2.0);
    assert!(!series.nonlinear);
    // A series switch skips the elements under [On] and [Off].
    let switch = ibs.model("switch").unwrap();
    assert_eq!(switch.series, Default::default());
    let skipped: Vec<_> = ibs.skipped.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        skipped,
        [
            "[On]",
            "[R Series]",
            "[Off]",
            "[R Series]",
            "[Series MOSFET]"
        ]
    );
}

/// Parses a file made of the header and the given lines, and returns the error.
fn error(body: &str) -> (usize, String) {
    let text = format!("[IBIS Ver] 4.0\n{body}");
    let err = parse(&text).unwrap_err();
    (err.line, err.message)
}

#[test]
fn test_errors() {
    assert_eq!(
        parse("[File Name] x.ibs\n").unwrap_err().message,
        "missing [IBIS Ver]"
    );
    let cases: &[(&str, usize, &str)] = &[
        ("[IBIS Ver] 4.0\n", 2, "duplicate [IBIS Ver]"),
        ("[Pin]\n1 A GND\n", 2, "[Pin] outside of a [Component]"),
        (
            "[Model] m\nModel_type Input\n[Pin]\n",
            4,
            "[Pin] outside of a [Component]",
        ),
        ("[Pullup]\n0 0\n", 2, "[Pullup] outside of a [Model]"),
        ("[Component] c\n[Ramp]\n", 3, "[Ramp] outside of a [Model]"),
        ("[Component]\n", 2, "[Component] needs a value"),
        (
            "[Component] c\n[Component] c\n",
            3,
            "duplicate [Component] c",
        ),
        ("[Component] c\n[Pin]\n1 A\n", 4, "a row of [Pin] needs"),
        (
            "[Component] c\n[Pin]\n1 A GND\n1 B GND\n",
            5,
            "duplicate pin 1",
        ),
        (
            "[Component] c\n[Pin]\n1 A GND x NA NA\n",
            4,
            "invalid value `x` in [Pin]",
        ),
        ("[Component] c\n[Pin]\n1 A m\n", 4, "the pin refers to `m`"),
        (
            "[Component] c\n[Package]\nR_pkg 1 2\n",
            4,
            "R_pkg needs the typical",
        ),
        (
            "[Component] c\n[Package]\nR_pkg NA 1 2\n",
            4,
            "the typical value of R_pkg is NA",
        ),
        ("[Model] m\n", 2, "[Model] m has no Model_type"),
        (
            "[Model] m\nModel_type Bidir\n",
            3,
            "unknown Model_type `Bidir`",
        ),
        (
            "[Model] m\nModel_type Input\nModel_type Input\n",
            4,
            "duplicate Model_type",
        ),
        (
            "[Model] m\nModel_type Input\nPolarity up\n",
            4,
            "unknown Polarity",
        ),
        (
            "[Model] m\nModel_type Input\nEnable on\n",
            4,
            "unknown Enable",
        ),
        (
            "[Model] m\nModel_type Input\nVinl = x\n",
            4,
            "invalid value `x` for Vinl",
        ),
        (
            "[Model] m\nModel_type Input\nVinl 1 2\n",
            4,
            "Vinl needs one value",
        ),
        (
            "[Model] m\nModel_type Input\n[Model] m\nModel_type Input\n",
            4,
            "duplicate [Model] m",
        ),
        (
            "[Model] m\nModel_type Output\n[Rising Waveform]\nV_fixture = 0\n0 0\n1n 1\n",
            4,
            "[Rising Waveform] needs R_fixture",
        ),
        (
            "[Model] m\nModel_type Output\n[Falling Waveform]\nR_fixture 50\nV_fixture 0\n0 0\n",
            4,
            "[Falling Waveform] needs at least two rows",
        ),
        (
            "[Model] m\nModel_type Output\n[Rising Waveform]\nR_fixture 50\nV_fixture 0\n1n 0\n1n 1\n",
            4,
            "the times of [Rising Waveform] must increase: 0.000000001 after 0.000000001",
        ),
        (
            "[Model] m\nModel_type Output\n[Rising Waveform]\nR_fixture 50\nV_fixture 0\n0 0 1\n1n 1\n",
            7,
            "[Rising Waveform] needs the typical",
        ),
        (
            "[Model] m\nModel_type Input\n[Voltage Range] 3.3 x NA\n",
            4,
            "invalid value `x` for [Voltage Range]",
        ),
        (
            "[Model] m\nModel_type Input\n[GND Clamp]\n0 0\n[GND Clamp]\n0 0\n",
            6,
            "duplicate [GND Clamp]",
        ),
        (
            "[Model] m\nModel_type Input\n[GND Clamp]\n",
            4,
            "[GND Clamp] has no rows",
        ),
        (
            "[Model] m\nModel_type Input\n[GND Clamp]\nx 0 NA NA\n",
            5,
            "invalid voltage `x` in [GND Clamp]",
        ),
        (
            "[Model] m\nModel_type Input\n[GND Clamp]\n0 NA 0 0\n1 NA 1 1\n",
            4,
            "the typical column of [GND Clamp] is all NA",
        ),
        (
            "[Component] c\n[Pin]\n1 A GND\n[Diff Pin]\n1 2 0.1\n",
            6,
            "[Diff Pin] names the pin 2, which is not in [Pin]",
        ),
        (
            "[Component] c\n[Pin]\n1 A GND\n2 B GND\n3 C GND\n[Diff Pin]\n1 2 0.1\n3 1 0.1\n",
            9,
            "the pin 1 is in two rows of [Diff Pin]",
        ),
        (
            "[Component] c\n[Pin]\n1 A GND\n2 B GND\n[Diff Pin]\n1 2\n",
            7,
            "a row of [Diff Pin] needs",
        ),
        (
            "[Component] c\n[Pin]\n1 A GND\n2 B GND\n[Diff Pin]\n1 2 x\n",
            7,
            "invalid value `x` in [Diff Pin]",
        ),
        (
            "[Component] c\n[Pin]\n1 A GND\n2 B GND\n[Series Pin Mapping]\n1 2 s\n",
            7,
            "the pin refers to `s`",
        ),
        (
            "[Component] c\n[Pin]\n1 A GND\n[Series Pin Mapping]\n1 2 s\n",
            6,
            "[Series Pin Mapping] names the pin 2",
        ),
        (
            "[Diff Pin]\n1 2 0.1\n",
            2,
            "[Diff Pin] outside of a [Component]",
        ),
        (
            "[Model] m\nModel_type Series\n[R Series] 1\n[R Series] 2\n",
            5,
            "duplicate [R Series]",
        ),
        ("[Model Selector] s\n", 2, "[Model Selector] has no models"),
        (
            "[Model Selector] s\nm one\n[Model Selector] s\nm two\n",
            4,
            "duplicate [Model Selector] s",
        ),
        (
            "[Model Selector] s\nm the model\nx another\n[Model] m\nModel_type Input\n",
            4,
            "[Model Selector] s lists `x`, which is not a [Model] of the file",
        ),
        (
            "[Model] m\nModel_type Output\n[Ramp]\ndV/dt_r 1/1n NA NA\n",
            4,
            "[Ramp] needs both",
        ),
        (
            "[Model] m\nModel_type Output\n[Ramp]\ndV/dt_r 1 NA NA\n",
            5,
            "invalid value `1` for dV/dt_r",
        ),
    ];
    for &(body, line, message) in cases {
        let (l, m) = error(body);
        assert!(
            l == line && m.starts_with(message),
            "{body:?}: got line {l}: {m}; expected line {line}: {message}"
        );
    }
}

#[test]
fn test_model_type_names() {
    for name in ["I/O_open_drain", "3-state_ECL", "Series_switch", "Input"] {
        let t = ModelType::from_name(&name.to_lowercase()).unwrap();
        assert_eq!(t.name(), name);
        assert_eq!(t.to_string(), name);
    }
    assert_eq!(ModelType::from_name("IO"), None);
}

/// The IBIS file of the KiCad demo `simulation/ibis`, which tests odd syntax: a comment on the
/// `[IBIS Ver]` line, `[Comment char]` in lowercase, `5mm` and `[GND_clamp]`.
#[test]
#[ignore = "needs the KiCad demos in /usr/share/kicad"]
fn test_kicad_demo() {
    let text =
        std::fs::read_to_string("/usr/share/kicad/demos/simulation/ibis/ibis_v1_1.ibs").unwrap();
    let ibs = parse(&text).unwrap();
    assert_eq!(ibs.header.ibis_ver, "1.1");
    assert_eq!(ibs.header.comment_char, '#');
    assert_eq!(
        ibs.header.source.as_deref(),
        Some("This is the\nsource for the files")
    );
    let c = ibs.component("Virtual").unwrap();
    assert_eq!(c.manufacturer.as_deref(), Some("KiCad"));
    assert_eq!(c.pins.len(), 4);
    approx(c.pin("1").unwrap().r_pin.unwrap(), 5e-3);
    let input = ibs.model("Input").unwrap();
    assert_eq!(input.model_type, ModelType::Input);
    assert_eq!((input.vinl, input.vinh), (Some(0.8), Some(2.0)));
    assert_eq!(input.gnd_clamp.as_ref().unwrap().points.len(), 3);
    let output = ibs.model("Output").unwrap();
    assert_eq!(output.model_type, ModelType::Output);
    assert_eq!(output.pulldown.as_ref().unwrap().points.len(), 4);
    let ramp = output.ramp.unwrap();
    approx(ramp.dv_dt_r.typ.dt, 30e-9);
    assert_eq!(ramp.dv_dt_r.max, None);
    assert!(ibs.skipped.is_empty(), "{:?}", ibs.skipped);
}
