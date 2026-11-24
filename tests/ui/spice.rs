// The value of a resistor is not a number, so it cannot be simulated.
cuprite_wire::kicad_part! {
    struct Resistor {
        symbol: "test:R",
        footprint: "test:R_0603",
    }
}

// Skipping the simulation is fine.
cuprite_wire::kicad_part! {
    struct NotSimulated {
        symbol: "test:R",
        footprint: "test:R_0603",
        spice: none,
    }
}

// Writing it by hand needs `CustomSpice`.
cuprite_wire::kicad_part! {
    struct NoCustomSpice {
        symbol: "test:R",
        footprint: "test:R_0603",
        spice: custom,
    }
}

// A model and `spice: custom` contradict each other.
cuprite_wire::kicad_part! {
    struct CustomAndSim {
        symbol: "test:VSUP",
        spice: custom,
        sim: { library: "${TEST_MODELS}/clamp.lib", name: "clamp", pins: { "1" => "A", "2" => "K" } },
    }
}

fn main() {}
