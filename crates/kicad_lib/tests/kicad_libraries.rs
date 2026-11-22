//! Reads parts from the KiCad (9 or 10) libraries installed in the devcontainer.
//! Run with `cargo test -p cuprite_wire_kicad_lib -- --ignored`.

use cuprite_wire_kicad_lib::{
    part::{CommonOptions, FootprintOption, PartOptions, load_part},
    resolve::Resolver,
    sim::{Args, Elements, ModelDef, SpiceDef, Value},
};

fn resolver() -> Resolver {
    Resolver::new(env!("CARGO_MANIFEST_DIR"), |name| std::env::var(name).ok())
}

fn options(symbol: &str, footprint: Option<&str>) -> PartOptions {
    PartOptions {
        common: CommonOptions {
            type_name: "T".into(),
            ..Default::default()
        },
        symbol: symbol.into(),
        footprint: footprint.map_or(FootprintOption::None, |f| FootprintOption::Given(f.into())),
        ..Default::default()
    }
}

#[test]
#[ignore = "needs the KiCad libraries in /usr/share/kicad"]
fn test_kicad_parts() {
    let mut r = resolver();
    let mut opts = options("Device:R", Some("Resistor_SMD:R_0603_1608Metric"));
    opts.common.value = Some(Value::Param);
    let part = load_part(&opts, &mut r).unwrap();
    assert!(
        matches!(part.spice, SpiceDef::Elements(Elements { ref elements, .. }) if elements[0].device == 'R')
    );

    let part = load_part(
        &options(
            "Amplifier_Operational:LM358",
            Some("Package_SO:SOIC-8_3.9x4.9mm_P1.27mm"),
        ),
        &mut r,
    )
    .unwrap();
    assert_eq!(part.pins.len(), 8);
    assert_eq!(part.spice, SpiceDef::Unsupported);

    let error = load_part(
        &options(
            "Amplifier_Operational:LM358",
            Some("Package_SO:SOIC-8-1EP_3.9x4.9mm_P1.27mm_EP2.41x3.3mm"),
        ),
        &mut r,
    )
    .unwrap_err();
    assert!(
        error.error.message.contains("pads without a symbol pin: 9"),
        "{}",
        error.error
    );

    let part = load_part(&options("Simulation_SPICE:OPAMP", None), &mut r).unwrap();
    let SpiceDef::Elements(Elements { elements, .. }) = part.spice else {
        panic!()
    };
    let element = &elements[0];
    assert_eq!(element.device, 'X');
    assert_eq!(element.args, Args::Fixed("kicad_builtin_opamp".into()));
    assert!(
        matches!(&element.models[0], ModelDef::Include(path) if path.ends_with("Simulation_SPICE.sp"))
    );

    for symbol in [
        "Simulation_SPICE:D",
        "Simulation_SPICE:NPN",
        "Simulation_SPICE:NMOS",
        "Simulation_SPICE:PMOS_Substrate",
        "Simulation_SPICE:NJFET",
        "Simulation_SPICE:VDC",
        "Device:D",
    ] {
        let part = load_part(&options(symbol, None), &mut r)
            .unwrap_or_else(|e| panic!("{symbol}: {}", e.error));
        assert!(
            matches!(part.spice, SpiceDef::Elements(Elements { .. })),
            "{symbol}"
        );
    }
}
