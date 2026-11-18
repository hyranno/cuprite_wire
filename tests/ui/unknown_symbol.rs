cuprite_wire::kicad_part! {
    struct Missing {
        symbol: "test:MISSING",
        footprint: "test:R_0603",
    }
}

cuprite_wire::kicad_part! {
    struct NoLibrary {
        symbol: "nolib:R",
        footprint: "test:R_0603",
    }
}

cuprite_wire::kicad_part! {
    struct Ground {
        symbol: "test:GND",
        footprint: "test:R_0603",
    }
}

fn main() {}
