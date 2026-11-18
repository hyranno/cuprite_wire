// Errors in the `sim` block of `kicad_part!`, with the TPS3899-like symbol test:VSUP
// (pins 1 CTR, 2 CTS, 3 GND, 4 VDD, 5 SENSE, 6 ~{RESET_N}) and the model VSUP01
// (ports CTR CTS GND RESET_B SENSE VDD).

// 3: a port without a pin, with the mapping suggested from the names.
cuprite_wire::kicad_part! {
    struct UnmappedPort {
        symbol: "test:VSUP",
        sim: {
            library: "${TEST_MODELS}/vsup01.lib",
            name: "VSUP01",
            pins: { "1" => "CTR", "2" => "CTS", "3" => "GND", "4" => "VDD", "5" => "SENSE" },
        },
    }
}

// 2: a name that is not a port.
cuprite_wire::kicad_part! {
    struct UnknownPort {
        symbol: "test:VSUP",
        sim: {
            library: "${TEST_MODELS}/vsup01.lib",
            name: "VSUP01",
            pins: { "1" => "CTR", "2" => "CTS", "3" => "GND", "4" => "VDD", "5" => "SENSE", "6" => "RESET_N" },
        },
    }
}

// 2: a port mapped twice.
cuprite_wire::kicad_part! {
    struct PortTwice {
        symbol: "test:VSUP",
        sim: {
            library: "${TEST_MODELS}/vsup01.lib",
            name: "VSUP01",
            pins: { "1" => "CTR", "2" => "CTR", "3" => "GND", "4" => "VDD", "5" => "SENSE", "6" => "RESET_B" },
        },
    }
}

// 4: a pin of the part left out (the model has fewer ports than the symbol has pins).
cuprite_wire::kicad_part! {
    struct UnmappedPin {
        symbol: "test:VSUP",
        sim: {
            library: "${TEST_MODELS}/vsup_unsupported.lib",
            name: "VSUP_MIXED",
            pins: { "5" => "IN", "6" => "OUT" },
            dialect: pspice,
            unused: ["1", "2"],
        },
    }
}

// 5: a parameter the subcircuit does not have.
cuprite_wire::kicad_part! {
    struct UnknownParam {
        symbol: "test:VSUP",
        sim: {
            library: "${TEST_MODELS}/vsup01.lib",
            name: "VSUP01",
            pins: { "1" => "CTR", "2" => "CTS", "3" => "GND", "4" => "VDD", "5" => "SENSE", "6" => "RESET_B" },
            params: { "TD" => "1m" },
        },
    }
}

// 6: models ngspice cannot use.
cuprite_wire::kicad_part! {
    struct Encrypted {
        symbol: "test:VSUP",
        sim: {
            library: "${TEST_MODELS}/vsup_unsupported.lib",
            name: "VSUP_LOCKED",
            pins: { "5" => "IN", "6" => "OUT" },
            unused: ["1", "2", "3", "4"],
        },
    }
}

cuprite_wire::kicad_part! {
    struct Optional {
        symbol: "test:VSUP",
        sim: {
            library: "${TEST_MODELS}/vsup_unsupported.lib",
            name: "VSUP_DIGITAL",
            pins: { "5" => "IN", "6" => "OUT" },
            unused: ["1", "2", "3", "4"],
        },
    }
}

// 7: PSpice and LTspice constructs in one library.
cuprite_wire::kicad_part! {
    struct MixedDialects {
        symbol: "test:VSUP",
        sim: {
            library: "${TEST_MODELS}/vsup_unsupported.lib",
            name: "VSUP_MIXED",
            pins: { "5" => "IN", "6" => "OUT" },
            unused: ["1", "2", "3", "4"],
        },
    }
}

// Several models need distinct suffixes.
cuprite_wire::kicad_part! {
    struct Suffixes {
        symbol: "test:VSUP",
        sim: [
            { library: "${TEST_MODELS}/clamp.lib", name: "clamp", pins: { "1" => "A", "2" => "K" } },
            { library: "${TEST_MODELS}/clamp.lib", name: "clamp", pins: { "3" => "A", "4" => "K" } },
        ],
        footprint: none,
    }
}

// A model and `spice: none` contradict each other.
cuprite_wire::kicad_part! {
    struct NotSimulated {
        symbol: "test:VSUP",
        spice: none,
        sim: { library: "${TEST_MODELS}/clamp.lib", name: "clamp", pins: { "1" => "A", "2" => "K" } },
    }
}

fn main() {}
