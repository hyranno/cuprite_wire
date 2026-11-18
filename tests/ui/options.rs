cuprite_wire::kicad_part! {
    struct UnknownKey {
        symbol: "test:R",
        colour: "red",
    }
}

cuprite_wire::kicad_part! {
    struct NoSymbol {
        footprint: "test:R_0603",
    }
}

cuprite_wire::kicad_part! {
    struct Twice {
        symbol: "test:R",
        symbol: "test:R",
    }
}

cuprite_wire::kicad_part! {
    struct BadKeyword {
        symbol: "test:R",
        spice: maybe,
    }
}

cuprite_wire::kicad_part! {
    struct BadPins {
        symbol: "test:R",
        footprint: "test:R_0603",
        value: param,
        pins: { "1" => "a", "3" => "b" },
    }
}

cuprite_wire::kicad_part! {
    struct BadGroup {
        symbol: "test:R",
        footprint: "test:R_0603",
        value: param,
        internal_connections: [["1", "x"]],
    }
}

fn main() {}
