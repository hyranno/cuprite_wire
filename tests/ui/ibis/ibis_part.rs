// Errors of `ibis_part!`, each pointing at the option concerned.

// The file has two components.
cuprite_wire::ibis_part! {
    struct NoComponent {
        library: "${TEST_MODELS}/ibis_part.ibs",
        pins: { tx => "3" },
    }
}

cuprite_wire::ibis_part! {
    struct UnknownComponent {
        library: "${TEST_MODELS}/ibis_part.ibs",
        component: "chip",
    }
}

// Pins are named by number, not by signal.
cuprite_wire::ibis_part! {
    struct BySignal {
        library: "${TEST_MODELS}/ibis_part.ibs",
        component: "Chip",
        pins: { tx => "TX" },
    }
}

// The models of the selector are listed.
cuprite_wire::ibis_part! {
    struct UnknownModel {
        library: "${TEST_MODELS}/ibis_part.ibs",
        component: "Chip",
        pins: { io => { pin: "4", model: "fast" } },
    }
}

cuprite_wire::ibis_part! {
    struct CannotSimulate {
        library: "${TEST_MODELS}/ibis_part.ibs",
        component: "Chip",
        pins: { bad => "6" },
    }
}

cuprite_wire::ibis_part! {
    struct NotAPair {
        library: "${TEST_MODELS}/ibis_part.ibs",
        component: "Chip",
        diff_pins: { tx => "3" },
    }
}

cuprite_wire::ibis_part! {
    struct NoSeries {
        library: "${TEST_MODELS}/ibis_part.ibs",
        component: "Chip",
        series: { term => ["7", "9"] },
    }
}

cuprite_wire::ibis_part! {
    struct Twice {
        library: "${TEST_MODELS}/ibis_part.ibs",
        component: "Chip",
        pins: { tx => "3" },
        diff_pins: { tx => "1" },
    }
}

cuprite_wire::ibis_part! {
    struct Missing {
        library: "${TEST_MODELS}/none.ibs",
    }
}

cuprite_wire::ibis_part! {
    struct NoLibrary {
        component: "Chip",
    }
}

fn main() {}
