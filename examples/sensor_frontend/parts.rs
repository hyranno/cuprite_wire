//! Parts generated from the example's own libraries in `libs/` at compile time.

use cuprite_wire::kicad_part;

kicad_part! {
    /// Rail-to-rail single op-amp in SOT-23-5.
    ///
    /// The symbol has no `Sim.*` fields (as is common for vendor symbols), so the SPICE model is
    /// attached here with a `sim` block, mapping each symbol pin to a port of the `.subckt` by name.
    /// The footprint comes from the cuprite_wire basic library.
    pub struct RrioOpAmp {
        symbol: "sensor_frontend:OPAMP_RRIO",
        pins: { "1" => "out", "2" => "vee", "3" => "in_p", "4" => "in_n", "5" => "vcc" },
        sim: {
            library: "examples/sensor_frontend/libs/models/rrio_opamp.lib",
            name: "rrio_opamp",
            pins: { "1" => "out", "2" => "vee", "3" => "inp", "4" => "inn", "5" => "vcc" },
        },
        symbol_dir: "examples/sensor_frontend/libs/symbols",
        footprint_dir: "libs/footprints",
    }
}

kicad_part! {
    /// 1x04 pin header bringing the supply and the signals onto the board.
    /// The symbol is excluded from simulation; test benches drive the nets instead.
    pub struct Header1x04 {
        symbol: "sensor_frontend:CONN_01x04",
        symbol_dir: "examples/sensor_frontend/libs/symbols",
        footprint_dir: "examples/sensor_frontend/libs/footprints",
    }
}
