// Errors of `kicad_part!` with IBIS models: the symbols test:IBIS_LEGACY (like KiCad's
// Simulation_SPICE:IBIS_DRIVER, pins 1 REF and 2) and test:IBIS_LEGACY_DIFF (pins 1 REF, 2 +,
// 3 -), and the file kicad_ibis.ibs (component Board: 1 TX out_3s, 2 RX in_cmos, 3/4 a pair of
// out_3s, 5 IO with the selector io_sel, 8 DRV out_pp).

// KiCad's library symbols have an older Sim.Type and no file.
cuprite_wire::kicad_part! {
    struct Legacy {
        symbol: "test:IBIS_LEGACY",
        footprint: none,
    }
}

cuprite_wire::kicad_part! {
    struct NoPin {
        symbol: "test:IBIS_LEGACY",
        footprint: none,
        ibis: { library: "${TEST_MODELS}/kicad_ibis.ibs" },
    }
}

cuprite_wire::kicad_part! {
    struct UnknownPin {
        symbol: "test:IBIS_LEGACY",
        footprint: none,
        ibis: { library: "${TEST_MODELS}/kicad_ibis.ibs", pin: "9" },
    }
}

cuprite_wire::kicad_part! {
    struct UnknownComponent {
        symbol: "test:IBIS_LEGACY",
        footprint: none,
        ibis: { library: "${TEST_MODELS}/kicad_ibis.ibs", component: "board", pin: "1" },
    }
}

cuprite_wire::kicad_part! {
    struct UnknownModel {
        symbol: "test:IBIS_LEGACY",
        footprint: none,
        ibis: { library: "${TEST_MODELS}/kicad_ibis.ibs", pin: "5", model: "out_pp" },
    }
}

// An input cannot drive.
cuprite_wire::kicad_part! {
    struct InputDriver {
        symbol: "test:IBIS_LEGACY",
        footprint: none,
        ibis: { library: "${TEST_MODELS}/kicad_ibis.ibs", pin: "2", driver: true },
    }
}

// A pair needs the three pins GND, + and -.
cuprite_wire::kicad_part! {
    struct PairOnTwoPins {
        symbol: "test:IBIS_LEGACY",
        footprint: none,
        ibis: { library: "${TEST_MODELS}/kicad_ibis.ibs", pin: "3", diff: true },
    }
}

cuprite_wire::kicad_part! {
    struct ValueParam {
        symbol: "test:IBIS_LEGACY",
        footprint: none,
        value: param,
        ibis: { library: "${TEST_MODELS}/kicad_ibis.ibs", pin: "1" },
    }
}

cuprite_wire::kicad_part! {
    struct NotSimulated {
        symbol: "test:IBIS_LEGACY",
        footprint: none,
        spice: none,
        ibis: { library: "${TEST_MODELS}/kicad_ibis.ibs", pin: "1" },
    }
}

fn main() {}
