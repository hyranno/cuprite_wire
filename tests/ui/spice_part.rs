// Errors of `spice_part!`.

// 1: a name that is not in the library; similar names and the subcircuits nothing else uses are listed.
cuprite_wire::spice_part! {
    struct UnknownName {
        library: "${TEST_MODELS}/vsup01.lib",
        name: "VSUP1",
    }
}

// `pins` renames ports.
cuprite_wire::spice_part! {
    struct UnknownPort {
        library: "${TEST_MODELS}/vsup01.lib",
        name: "VSUP01",
        pins: { "RESET" => "reset_n" },
    }
}

// 5: parameters are those of the .subckt line.
cuprite_wire::spice_part! {
    struct UnknownParam {
        library: "${TEST_MODELS}/vsup01.lib",
        name: "VSUP01",
        params: { "VTH" => "0.6", "TD" => "1m" },
    }
}

// `.LIB` is read only in ngspice's PSpice mode.
cuprite_wire::spice_part! {
    struct WrongDialect {
        library: "${TEST_MODELS}/vsup01.lib",
        name: "VSUP01",
        dialect: ngspice,
    }
}

cuprite_wire::spice_part! {
    struct NoName {
        library: "${TEST_MODELS}/vsup01.lib",
    }
}

cuprite_wire::spice_part! {
    struct SimKey {
        library: "${TEST_MODELS}/vsup01.lib",
        name: "VSUP01",
        unused: ["CTR"],
    }
}

fn main() {}
