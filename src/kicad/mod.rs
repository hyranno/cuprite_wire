//! KiCad-specific part information and netlist export.
//!
//! See the guide [Exporting a KiCad netlist](https://github.com/hyranno/cuprite_wire/blob/main/doc/kicad_netlist_usage.md).

pub mod netlist;
pub mod part;

pub use netlist::KicadNetlist;
pub use part::{
    KicadPartInfo, KicadPinInfo, LibId, LibIdError, PartFlags, PinInfoProblem, PinType,
};
