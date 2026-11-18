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

fn main() {}
