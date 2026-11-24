//! Overriding the value, footprint, fields and flags of a part instance, e.g. marking it DNP.

use std::{any::Any, collections::BTreeMap};

use crate::{
    kicad::{KicadPartInfo, LibId},
    parts::{Part, PinType},
    spice::SpicePartInfo,
};

/// What [`Overridden`] replaces in the information of a part. `None` (or no entry) keeps the
/// part's own.
///
/// Only the information written to the netlist and the bill of materials is overridden, not the
/// pins nor the SPICE information: [`PartOverrides::value`] does not change what is simulated.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PartOverrides {
    /// The value ([`Part::value`]).
    pub value: Option<String>,
    /// The footprint ([`KicadPartInfo::footprint`]).
    pub footprint: Option<LibId>,
    /// Fields ([`KicadPartInfo::fields`]): one with the name of a field of the part replaces its
    /// value, the others are appended in order.
    pub fields: Vec<(String, String)>,
    /// Do not populate ([`PartFlags::dnp`](crate::kicad::PartFlags::dnp)).
    pub dnp: Option<bool>,
    /// Excluded from the bill of materials
    /// ([`PartFlags::exclude_from_bom`](crate::kicad::PartFlags::exclude_from_bom)).
    pub exclude_from_bom: Option<bool>,
    /// Not placed on the board
    /// ([`PartFlags::exclude_from_board`](crate::kicad::PartFlags::exclude_from_board)).
    pub exclude_from_board: Option<bool>,
}

impl PartOverrides {
    /// `info` with the overrides applied.
    pub fn apply(&self, mut info: KicadPartInfo) -> KicadPartInfo {
        if let Some(footprint) = &self.footprint {
            info.footprint = Some(footprint.clone());
        }
        for (name, value) in &self.fields {
            match info.fields.iter_mut().find(|(field, _)| field == name) {
                Some((_, old)) => old.clone_from(value),
                None => info.fields.push((name.clone(), value.clone())),
            }
        }
        let flags = &mut info.flags;
        flags.dnp = self.dnp.unwrap_or(flags.dnp);
        flags.exclude_from_bom = self.exclude_from_bom.unwrap_or(flags.exclude_from_bom);
        flags.exclude_from_board = self.exclude_from_board.unwrap_or(flags.exclude_from_board);
        info
    }
}

/// A part with some of its information overridden, made by the methods of [`PartExt`] (e.g.
/// `part.dnp()`). Everything not in [`Overridden::overrides`] is the part's own.
///
/// The KiCad overrides are ignored for a part that cannot be exported to KiCad
/// ([`Part::kicad`] is `None`).
#[derive(Debug, Clone)]
pub struct Overridden {
    /// The part.
    pub part: Box<dyn Part>,
    /// What is overridden.
    pub overrides: PartOverrides,
}

impl Part for Overridden {
    fn ref_prefix(&self) -> String {
        self.part.ref_prefix()
    }
    fn pin_ids(&self) -> Vec<String> {
        self.part.pin_ids()
    }
    fn value(&self) -> String {
        self.overrides
            .value
            .clone()
            .unwrap_or_else(|| self.part.value())
    }
    fn internal_connections(&self) -> Vec<Vec<String>> {
        self.part.internal_connections()
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        self.part.kicad().map(|info| self.overrides.apply(info))
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        self.part.spice()
    }
    #[cfg(feature = "shared-ngspice")]
    fn cosim_device(&self) -> Option<&crate::spice::cosim::CosimDevice> {
        self.part.cosim_device()
    }
    fn extra_pin_types(&self) -> BTreeMap<String, PinType> {
        self.part.extra_pin_types()
    }
}

/// Overriding the information of a part instance, implemented for every part and
/// `Box<dyn Part>`:
///
/// ```
/// use cuprite_wire::{basic::Resistor0603, circuit::CircuitBuilder, parts::PartExt};
///
/// let mut c: CircuitBuilder = CircuitBuilder::new();
/// c.instantiate_part(Resistor0603::new("10k").dnp().with_field("MPN", "RC0603FR-0710KL"));
/// ```
///
/// The methods return an [`Overridden`]; called on one, they add to its overrides (a later
/// override of the same item wins).
pub trait PartExt: Sized {
    /// The part as an [`Overridden`], with no overrides unless it is one.
    fn into_overridden(self) -> Overridden;

    /// The part with the value `value`. It does not change what is simulated.
    fn with_value(self, value: impl Into<String>) -> Overridden {
        let mut part = self.into_overridden();
        part.overrides.value = Some(value.into());
        part
    }
    /// The part with the footprint `footprint`.
    fn with_footprint(self, footprint: LibId) -> Overridden {
        let mut part = self.into_overridden();
        part.overrides.footprint = Some(footprint);
        part
    }
    /// The part with the field `name` set to `value` (replaced, or appended if the part has no
    /// such field).
    fn with_field(self, name: impl Into<String>, value: impl Into<String>) -> Overridden {
        let mut part = self.into_overridden();
        let (name, value) = (name.into(), value.into());
        let fields = &mut part.overrides.fields;
        match fields.iter_mut().find(|(field, _)| *field == name) {
            Some((_, old)) => *old = value,
            None => fields.push((name, value)),
        }
        part
    }
    /// The part marked as not populated (DNP). It is still simulated.
    fn dnp(self) -> Overridden {
        let mut part = self.into_overridden();
        part.overrides.dnp = Some(true);
        part
    }
    /// The part excluded from the bill of materials.
    fn exclude_from_bom(self) -> Overridden {
        let mut part = self.into_overridden();
        part.overrides.exclude_from_bom = Some(true);
        part
    }
    /// The part marked as not placed on the board.
    fn exclude_from_board(self) -> Overridden {
        let mut part = self.into_overridden();
        part.overrides.exclude_from_board = Some(true);
        part
    }
}

impl<P: Part + 'static> PartExt for P {
    fn into_overridden(self) -> Overridden {
        match (Box::new(self) as Box<dyn Any>).downcast::<Overridden>() {
            Ok(part) => *part,
            Err(part) => Overridden {
                part: part.downcast::<P>().expect("the part is a `P`"),
                overrides: PartOverrides::default(),
            },
        }
    }
}

impl PartExt for Box<dyn Part> {
    fn into_overridden(self) -> Overridden {
        Overridden {
            part: self,
            overrides: PartOverrides::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        circuit::CircuitBuilder,
        kicad::PartFlags,
        test_parts::{CustomPart, Resistor},
    };

    fn custom() -> CustomPart {
        let mut part = CustomPart::valid();
        part.kicad.as_mut().unwrap().fields = vec![("MPN".into(), "A".into())];
        part
    }

    #[test]
    fn test_overrides() {
        let part = custom()
            .with_value("v")
            .with_field("MPN", "B")
            .with_field("Note", "x")
            .with_footprint(LibId::new("Lib", "Fp").unwrap())
            .dnp()
            .with_field("Note", "y");
        // Chained calls make one `Overridden`.
        assert!(format!("{:?}", part.part).starts_with("CustomPart"));
        assert_eq!(part.value(), "v");
        let info = part.kicad().unwrap();
        assert_eq!(info.footprint, Some(LibId::new("Lib", "Fp").unwrap()));
        assert_eq!(
            info.fields,
            [("MPN".into(), "B".into()), ("Note".into(), "y".into())]
        );
        assert_eq!(
            info.flags,
            PartFlags {
                dnp: true,
                ..Default::default()
            }
        );
        // The rest is the part's own.
        let original = custom();
        assert_eq!(info.pins, original.kicad().unwrap().pins);
        assert_eq!(part.pin_ids(), original.pin_ids());
        assert_eq!(part.spice(), original.spice());
    }

    #[test]
    fn test_boxed_and_flags() {
        let boxed: Box<dyn Part> = Box::new(Resistor("1k"));
        let part = boxed.exclude_from_bom().exclude_from_board();
        assert_eq!(part.value(), "1k");
        let flags = part.kicad().unwrap().flags;
        assert!(!flags.dnp && flags.exclude_from_bom && flags.exclude_from_board);
    }

    #[test]
    fn test_in_bom() {
        let mut circuit = CircuitBuilder::new();
        circuit.instantiate_part(Resistor("10k"));
        circuit.instantiate_part(Resistor("10k").dnp());
        circuit.instantiate_part(Resistor("10k").with_field("MPN", "X"));
        let netlist = circuit.build(&()).to_netlist().unwrap().to_kicad().unwrap();
        let bom = netlist.bom(&crate::kicad::BomOptions::default().with_field("MPN"));
        assert_eq!(
            bom.to_string(),
            r#""Refs","Value","Footprint","Qty","DNP","MPN"
"R1","10k","Resistor_SMD:R_0603_1608Metric","1","",""
"R2","10k","Resistor_SMD:R_0603_1608Metric","1","DNP",""
"R3","10k","Resistor_SMD:R_0603_1608Metric","1","","X"
"#
        );
    }
}
