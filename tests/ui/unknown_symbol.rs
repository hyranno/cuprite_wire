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

// A part that failed is still a type, so that its uses add no "cannot find type" errors.
fn uses(_: Missing, _: NoLibrary, _: Ground) {}

fn main() {}
