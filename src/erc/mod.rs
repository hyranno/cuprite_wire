//! Electrical rules check (ERC) of a netlist, following the netlist-level rules of KiCad 10's ERC.
//!
//! The electrical types of pins come from [`Part::pin_types`](crate::parts::Part::pin_types).
//! Marks attached to nodes stand for the schematic items KiCad uses: a no-connect flag
//! ([`CircuitBuilder::no_connect`]), a `PWR_FLAG` ([`CircuitBuilder::power_flag`]), and pins outside the
//! circuit ([`CircuitBuilder::virtual_pin`]). A violation is excluded on one net with
//! [`CircuitBuilder::erc_allow`]. [`check_subcircuit`] checks a subcircuit alone.
//!
//! See the guide [Electrical rules check](https://github.com/hyranno/cuprite_wire/blob/main/doc/erc.md).
//!
//! [`CircuitBuilder::no_connect`]: crate::circuit::CircuitBuilder::no_connect
//! [`CircuitBuilder::power_flag`]: crate::circuit::CircuitBuilder::power_flag
//! [`CircuitBuilder::virtual_pin`]: crate::circuit::CircuitBuilder::virtual_pin
//! [`CircuitBuilder::erc_allow`]: crate::circuit::CircuitBuilder::erc_allow

mod rules;
mod subcircuit;
#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
};

pub use subcircuit::check_subcircuit;

use crate::{
    error::{CircuitWarningKind, Diagnostic},
    naming::case_insensitive_duplicates,
    netlist::{Net, Netlist},
    parts::PinType,
    trace::Trace,
};
use rules::{NetFinding, NetFlags, NetPin, check_net};

/// A mark attached to a node for ERC.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErcMark {
    /// The net is intentionally unconnected (KiCad's no-connect flag).
    NoConnect,
    /// A pin outside the circuit, seen by ERC as a pin of this type (`PWR_FLAG` is a `PowerOut` one).
    VirtualPin {
        /// The type ERC sees the pin as.
        pin_type: PinType,
        /// The name shown in ERC messages.
        name: String,
    },
    /// The violations of the rule on this net are not reported (like an excluded KiCad ERC marker).
    Allow(ErcRule),
}

/// A mark with the location where it was attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeMark {
    /// The mark.
    pub mark: ErcMark,
    /// Where the mark was attached, reported by violations about it.
    pub trace: Trace,
}

/// The severity of an ERC rule or a pin conflict, ordered from [`Severity::Ignore`] to [`Severity::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Not checked: violations are left out of the [`ErcReport`].
    Ignore,
    /// Reported as a warning.
    Warning,
    /// Reported as an error (see [`ErcReport::has_errors`]).
    Error,
}

/// Severities of connecting two pin types (KiCad's pin conflict matrix). Symmetric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinMap([[Severity; 12]; 12]);

impl PinMap {
    /// The severity of connecting `a` and `b`.
    pub fn get(&self, a: PinType, b: PinType) -> Severity {
        self.0[a.index()][b.index()]
    }

    /// Sets the severity of connecting `a` and `b` (in both orders).
    pub fn set(&mut self, a: PinType, b: PinType, severity: Severity) {
        self.0[a.index()][b.index()] = severity;
        self.0[b.index()][a.index()] = severity;
    }
}

impl Default for PinMap {
    /// KiCad's default (`erc.pin_map` of a new project, the same in KiCad 9 and 10).
    fn default() -> Self {
        use PinType::*;
        let mut map = PinMap([[Severity::Ignore; 12]; 12]);
        let warnings = [
            (Output, TriState),
            (Bidirectional, PowerOut),
            (Bidirectional, OpenEmitter),
            (TriState, PowerIn),
            (TriState, OpenCollector),
            (TriState, OpenEmitter),
        ];
        let errors = [
            (Output, Output),
            (Output, PowerOut),
            (Output, OpenCollector),
            (Output, OpenEmitter),
            (TriState, PowerOut),
            (PowerOut, PowerOut),
            (PowerOut, OpenCollector),
            (PowerOut, OpenEmitter),
        ];
        for (a, b) in warnings {
            map.set(a, b, Severity::Warning);
        }
        for (a, b) in errors {
            map.set(a, b, Severity::Error);
        }
        // An unspecified pin warns with anything but a free pin, and a no-connect pin is an error with anything.
        for other in PinType::ALL {
            if other != Free {
                map.set(Unspecified, other, Severity::Warning);
            }
        }
        for other in PinType::ALL {
            map.set(NoConnect, other, Severity::Error);
        }
        map
    }
}

/// The rules ERC checks, each with its severity in [`ErcSettings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ErcRule {
    /// Two connected pins whose types conflict (`pin_to_pin`). The severity of a conflict is given
    /// by [`PinMap`], and that of this rule is the highest it can be: [`Severity::Warning`] reports
    /// the errors of the pin map as warnings, and [`Severity::Ignore`] turns pin conflicts off.
    PinToPin,
    /// A pin is not connected to anything (`pin_not_connected`).
    PinNotConnected,
    /// An input pin has no driver on its net (`pin_not_driven`).
    PinNotDriven,
    /// A `power_in` pin has no `power_out` pin on its net (`power_pin_not_driven`).
    PowerPinNotDriven,
    /// A net marked no-connect connects several pins (`no_connect_connected`).
    NoConnectConnected,
    /// A no-connect mark is on a net without pins (`no_connect_dangling`).
    NoConnectDangling,
    /// A virtual pin is on a net without part pins.
    VirtualPinDangling,
    /// Net names differ only in case (`similar_labels`).
    SimilarNetNames,
    /// A net has several names (`multiple_net_names`, reported by [`Netlist`] too).
    MultipleNetNames,
    /// A labeled net has no part pins (`label_dangling`, reported by [`Netlist`] too).
    LabelOnlyNet,
    /// A labeled net has only one pin (`isolated_pin_label`, KiCad 10). Reported instead of
    /// [`ErcRule::PinNotConnected`].
    LabelSinglePin,
    /// A power pin named like a ground (`GND`, ...) is not on a ground net while another power pin
    /// of the part is (`ground_pin_not_ground`, KiCad 10).
    GroundPinNotGround,
}

impl ErcRule {
    /// The default severity, KiCad's except for [`ErcRule::LabelOnlyNet`] (an error in KiCad).
    pub fn default_severity(self) -> Severity {
        match self {
            ErcRule::PinToPin
            | ErcRule::PinNotConnected
            | ErcRule::PinNotDriven
            | ErcRule::PowerPinNotDriven => Severity::Error,
            ErcRule::NoConnectConnected
            | ErcRule::NoConnectDangling
            | ErcRule::VirtualPinDangling
            | ErcRule::SimilarNetNames
            | ErcRule::MultipleNetNames
            | ErcRule::LabelOnlyNet
            | ErcRule::LabelSinglePin
            | ErcRule::GroundPinNotGround => Severity::Warning,
        }
    }
}

/// The severities ERC uses; the default is KiCad 10's (see [`ErcRule::default_severity`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErcSettings {
    /// The severities of pin conflicts.
    pub pin_map: PinMap,
    /// Severities differing from [`ErcRule::default_severity`].
    severities: BTreeMap<ErcRule, Severity>,
}

impl ErcSettings {
    /// The severity of `rule`.
    pub fn severity(&self, rule: ErcRule) -> Severity {
        self.severities
            .get(&rule)
            .copied()
            .unwrap_or_else(|| rule.default_severity())
    }

    /// Sets the severity of `rule`.
    pub fn with_severity(mut self, rule: ErcRule, severity: Severity) -> Self {
        self.severities.insert(rule, severity);
        self
    }
}

/// A pin in an ERC message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErcPin {
    /// A pin of a part.
    Part {
        /// The reference designator of the part, e.g. `R1`.
        ref_name: String,
        /// The pin ID (the KiCad pin number).
        pin_id: String,
        /// The electrical type of the pin.
        pin_type: PinType,
    },
    /// A virtual pin ([`ErcMark::VirtualPin`]).
    Virtual {
        /// The name of the virtual pin.
        name: String,
        /// The type ERC sees the pin as.
        pin_type: PinType,
    },
}

impl ErcPin {
    /// The electrical type of the pin.
    pub fn pin_type(&self) -> PinType {
        match self {
            ErcPin::Part { pin_type, .. } | ErcPin::Virtual { pin_type, .. } => *pin_type,
        }
    }
}

impl fmt::Display for ErcPin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErcPin::Part {
                ref_name,
                pin_id,
                pin_type,
            } => write!(f, "{ref_name}.{pin_id} ({pin_type})"),
            ErcPin::Virtual { name, pin_type } => write!(f, "virtual pin {name:?} ({pin_type})"),
        }
    }
}

/// What an ERC violation is about. Each variant but [`ErcViolationKind::Netlist`] corresponds to
/// the [`ErcRule`] of the same name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErcViolationKind {
    /// [`ErcRule::PinToPin`]: two pins whose types conflict in the [`PinMap`]. Traced to `pin`,
    /// with a note at `other`.
    PinToPin {
        /// The name of the net.
        net: String,
        /// The reported pin.
        pin: ErcPin,
        /// The conflicting pin of the highest severity.
        other: ErcPin,
    },
    /// [`ErcRule::PinNotConnected`]. Traced to the pin.
    PinNotConnected {
        /// The name of the net.
        net: String,
        /// The only pin of the net.
        pin: ErcPin,
    },
    /// [`ErcRule::PinNotDriven`]. Traced to the pin.
    PinNotDriven {
        /// The name of the net.
        net: String,
        /// The input pin without a driver.
        pin: ErcPin,
    },
    /// [`ErcRule::PowerPinNotDriven`]. Traced to the pin.
    PowerPinNotDriven {
        /// The name of the net.
        net: String,
        /// A `power_in` pin of the net.
        pin: ErcPin,
    },
    /// [`ErcRule::NoConnectConnected`]. Traced to the no-connect mark, with a note at each pin.
    NoConnectConnected {
        /// The name of the net.
        net: String,
        /// The pins on the net.
        pins: Vec<ErcPin>,
    },
    /// [`ErcRule::NoConnectDangling`]. Traced to the mark.
    NoConnectDangling,
    /// [`ErcRule::VirtualPinDangling`]. Traced to the mark.
    VirtualPinDangling {
        /// The name of the virtual pin.
        name: String,
    },
    /// [`ErcRule::SimilarNetNames`]. Traced to the net `name`, with a note at the net `other`.
    SimilarNetNames {
        /// The reported net name.
        name: String,
        /// The first net name that differs from it only in case.
        other: String,
    },
    /// [`ErcRule::LabelSinglePin`]. Traced to the label, with a note at the pin.
    LabelSinglePin {
        /// The name of the net.
        net: String,
        /// The only pin of the net.
        pin: ErcPin,
    },
    /// [`ErcRule::GroundPinNotGround`]. Traced to the part.
    GroundPinNotGround {
        /// The name of the net the pin is on.
        net: String,
        /// The power pin.
        pin: ErcPin,
        /// The name of the pin.
        pin_name: String,
    },
    /// A warning of the netlist conversion ([`ErcRule::MultipleNetNames`] or [`ErcRule::LabelOnlyNet`]).
    Netlist(CircuitWarningKind),
}

impl fmt::Display for ErcViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErcViolationKind::PinToPin { net, pin, other } => write!(
                f,
                "net {net:?}: pins of type {} and {} are connected: {pin} and {other}",
                pin.pin_type(),
                other.pin_type()
            ),
            ErcViolationKind::PinNotConnected { net, pin } => {
                write!(f, "net {net:?}: {pin} is not connected to anything")
            }
            ErcViolationKind::PinNotDriven { net, pin } => {
                write!(
                    f,
                    "net {net:?}: input pin {pin} is not driven by any output pin"
                )
            }
            ErcViolationKind::PowerPinNotDriven { net, pin } => write!(
                f,
                "net {net:?}: {pin} is not driven by any power_out pin (add a `power_flag` if it is powered from outside)"
            ),
            ErcViolationKind::NoConnectConnected { net, pins } => {
                let pins: Vec<_> = pins.iter().map(ToString::to_string).collect();
                write!(
                    f,
                    "net {net:?} is marked no-connect but connects {}",
                    pins.join(", ")
                )
            }
            ErcViolationKind::NoConnectDangling => write!(f, "no-connect mark is not on any pin"),
            ErcViolationKind::VirtualPinDangling { name } => {
                write!(f, "virtual pin {name:?} is not connected to any part pin")
            }
            ErcViolationKind::SimilarNetNames { name, other } => {
                write!(f, "net names {name:?} and {other:?} differ only in case")
            }
            ErcViolationKind::LabelSinglePin { net, pin } => {
                write!(f, "label {net:?} is connected only to {pin}")
            }
            ErcViolationKind::GroundPinNotGround { net, pin, pin_name } => write!(
                f,
                "ground pin {pin_name:?} {pin} is on net {net:?}, which is not a ground net, while another power pin of the part is on one"
            ),
            ErcViolationKind::Netlist(kind) => kind.fmt(f),
        }
    }
}

/// A violation found by ERC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErcViolation {
    /// [`Severity::Warning`] or [`Severity::Error`]; ignored violations are not reported.
    pub severity: Severity,
    /// What the violation is, with its trace and notes.
    pub diagnostic: Diagnostic<ErcViolationKind>,
}

impl ErcViolation {
    /// What the violation is about.
    pub fn kind(&self) -> &ErcViolationKind {
        &self.diagnostic.kind
    }
}

impl fmt::Display for ErcViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning | Severity::Ignore => "warning",
        };
        self.diagnostic.fmt_with_severity(f, severity)
    }
}

/// The violations found by ERC, without the ignored ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErcReport {
    /// The reported violations, errors and warnings alike.
    pub violations: Vec<ErcViolation>,
}

impl ErcReport {
    /// The violations of [`Severity::Error`].
    pub fn errors(&self) -> impl Iterator<Item = &ErcViolation> {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Error)
    }

    /// The violations of [`Severity::Warning`].
    pub fn warnings(&self) -> impl Iterator<Item = &ErcViolation> {
        self.violations
            .iter()
            .filter(|v| v.severity == Severity::Warning)
    }

    /// Whether any violation is an error.
    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    /// Whether there is no violation at all, warnings included.
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    /// Fails a test if there is an error.
    ///
    /// # Panics
    /// If any violation is an error, with all violations (warnings included) in the message.
    #[track_caller]
    pub fn assert_no_errors(&self) {
        assert!(!self.has_errors(), "ERC found errors:\n{self}");
    }

    /// Fails a test if there is any violation.
    ///
    /// # Panics
    /// If there is any violation, warnings included, with all violations in the message.
    #[track_caller]
    pub fn assert_clean(&self) {
        assert!(self.is_clean(), "ERC found violations:\n{self}");
    }
}

impl fmt::Display for ErcReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, violation) in self.violations.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{violation}")?;
        }
        Ok(())
    }
}

impl Netlist {
    /// Checks the netlist. Its warnings are included in the report (as [`ErcRule::MultipleNetNames`]
    /// and [`ErcRule::LabelOnlyNet`]). The violations of the rules a net allows
    /// ([`ErcMark::Allow`]) are left out.
    pub fn erc(&self, settings: &ErcSettings) -> ErcReport {
        let mut report = Reporter {
            settings,
            violations: Vec::new(),
        };
        let pin_types: Vec<_> = self.components.iter().map(|c| c.part.pin_types()).collect();
        self.check_nets(&pin_types, &mut report);
        self.check_ground_pins(&pin_types, &mut report);
        self.check_pinless_marks(&mut report);
        self.check_similar_net_names(&mut report);
        self.report_warnings(&mut report);
        ErcReport {
            violations: report.violations,
        }
    }

    /// Checks the pins and marks on each net.
    fn check_nets(&self, pin_types: &[BTreeMap<String, PinType>], report: &mut Reporter) {
        // Pin ID -> index of its internal connection group, for each component.
        let stack_groups: Vec<HashMap<String, usize>> = self
            .components
            .iter()
            .map(|c| {
                let groups = c.part.internal_connections().into_iter().enumerate();
                groups
                    .flat_map(|(group, pins)| pins.into_iter().map(move |pin| (pin, group)))
                    .collect()
            })
            .collect();

        for net in &self.nets {
            let (pins, net_pins, traces) = self.net_pins(net, pin_types, &stack_groups);
            let no_connect = net
                .marks
                .iter()
                .find(|mark| mark.mark == ErcMark::NoConnect);
            let flags = NetFlags {
                no_connect: no_connect.is_some(),
                labeled: net.labeled,
            };
            for finding in check_net(&net_pins, flags, &report.settings.pin_map) {
                report.net_finding(net, finding, &pins, &traces, no_connect);
            }
        }
    }

    /// The pins on `net` (part pins, then virtual pins) for ERC, with the trace of each.
    fn net_pins(
        &self,
        net: &Net,
        pin_types: &[BTreeMap<String, PinType>],
        stack_groups: &[HashMap<String, usize>],
    ) -> (Vec<ErcPin>, Vec<NetPin>, Vec<Trace>) {
        let mut pins = Vec::new();
        let mut net_pins = Vec::new();
        let mut traces = Vec::new();
        for node in &net.nodes {
            let pin_type = pin_types[node.component]
                .get(&node.pin_id)
                .copied()
                .unwrap_or(PinType::Passive);
            // Pins not in an internal connection get their own group, after the real ones.
            let group = stack_groups[node.component]
                .get(&node.pin_id)
                .copied()
                .unwrap_or(usize::MAX - net_pins.len());
            let component = &self.components[node.component];
            pins.push(ErcPin::Part {
                ref_name: component.ref_name.clone(),
                pin_id: node.pin_id.clone(),
                pin_type,
            });
            net_pins.push(NetPin {
                pin_type,
                stack: Some((node.component, group)),
            });
            traces.push(component.trace.clone());
        }
        for mark in &net.marks {
            if let ErcMark::VirtualPin { pin_type, name } = &mark.mark {
                pins.push(ErcPin::Virtual {
                    name: name.clone(),
                    pin_type: *pin_type,
                });
                net_pins.push(NetPin {
                    pin_type: *pin_type,
                    stack: None,
                });
                traces.push(mark.trace.clone());
            }
        }
        (pins, net_pins, traces)
    }

    /// Reports the power pins named like a ground that are not on a ground net, on parts with
    /// another power pin on one (KiCad 10's `ERC_TESTER::TestGroundPins`).
    fn check_ground_pins(&self, pin_types: &[BTreeMap<String, PinType>], report: &mut Reporter) {
        let net_of: HashMap<(usize, &str), &Net> = self
            .nets
            .iter()
            .flat_map(|net| {
                net.nodes
                    .iter()
                    .map(move |node| ((node.component, node.pin_id.as_str()), net))
            })
            .collect();
        for (index, component) in self.components.iter().enumerate() {
            let Some(info) = component.part.kicad() else {
                continue;
            };
            let mut has_ground_net = false;
            let mut mismatched = Vec::new();
            for (pin_id, &pin_type) in &pin_types[index] {
                if !matches!(pin_type, PinType::PowerIn | PinType::PowerOut) {
                    continue;
                }
                let Some(&net) = net_of.get(&(index, pin_id.as_str())) else {
                    continue;
                };
                let net_is_ground = is_ground_name(&net.name);
                has_ground_net |= net_is_ground;
                let pin_name = info.pins.get(pin_id).and_then(|pin| pin.name.as_deref());
                if let Some(pin_name) = pin_name.filter(|name| is_ground_name(name))
                    && !net_is_ground
                {
                    mismatched.push((pin_id, pin_type, pin_name, net));
                }
            }
            if !has_ground_net {
                continue;
            }
            for (pin_id, pin_type, pin_name, net) in mismatched {
                if net.allows(ErcRule::GroundPinNotGround) {
                    continue;
                }
                let kind = ErcViolationKind::GroundPinNotGround {
                    net: net.name.clone(),
                    pin: ErcPin::Part {
                        ref_name: component.ref_name.clone(),
                        pin_id: pin_id.clone(),
                        pin_type,
                    },
                    pin_name: pin_name.to_string(),
                };
                report.push(
                    report.settings.severity(ErcRule::GroundPinNotGround),
                    Diagnostic::new(kind, component.trace.clone()),
                );
            }
        }
    }

    /// Reports the marks on nets without part pins.
    fn check_pinless_marks(&self, report: &mut Reporter) {
        for mark in &self.pinless_marks {
            let (rule, kind) = match &mark.mark {
                // Nothing to allow on a net without part pins.
                ErcMark::Allow(_) => continue,
                ErcMark::NoConnect => (
                    ErcRule::NoConnectDangling,
                    ErcViolationKind::NoConnectDangling,
                ),
                ErcMark::VirtualPin { name, .. } => (
                    ErcRule::VirtualPinDangling,
                    ErcViolationKind::VirtualPinDangling { name: name.clone() },
                ),
            };
            report.push(
                report.settings.severity(rule),
                Diagnostic::new(kind, mark.trace.clone()),
            );
        }
    }

    /// Reports net names that differ only in case.
    fn check_similar_net_names(&self, report: &mut Reporter) {
        for (i, first) in case_insensitive_duplicates(self.nets.iter().map(|net| net.name.as_str()))
        {
            let (net, other) = (&self.nets[i], &self.nets[first]);
            if net.allows(ErcRule::SimilarNetNames) || other.allows(ErcRule::SimilarNetNames) {
                continue;
            }
            let kind = ErcViolationKind::SimilarNetNames {
                name: net.name.clone(),
                other: other.name.clone(),
            };
            report.push(
                report.settings.severity(ErcRule::SimilarNetNames),
                Diagnostic::new(kind, net.trace.clone())
                    .with_note("the other net is named here", other.trace.clone()),
            );
        }
    }

    /// Reports the warnings of the netlist conversion.
    fn report_warnings(&self, report: &mut Reporter) {
        for warning in &self.warnings {
            let rule = match &warning.kind {
                CircuitWarningKind::MultipleNetNames { used, .. } => {
                    let net = self.nets.iter().find(|net| &net.name == used);
                    if net.is_some_and(|net| net.allows(ErcRule::MultipleNetNames)) {
                        continue;
                    }
                    ErcRule::MultipleNetNames
                }
                CircuitWarningKind::LabelOnlyNet { .. } => ErcRule::LabelOnlyNet,
            };
            let diagnostic = Diagnostic {
                kind: ErcViolationKind::Netlist(warning.kind.clone()),
                trace: warning.trace.clone(),
                notes: warning.notes.clone(),
            };
            report.push(report.settings.severity(rule), diagnostic);
        }
    }
}

struct Reporter<'a> {
    settings: &'a ErcSettings,
    violations: Vec<ErcViolation>,
}

impl Reporter<'_> {
    fn push(&mut self, severity: Severity, diagnostic: Diagnostic<ErcViolationKind>) {
        if severity != Severity::Ignore {
            self.violations.push(ErcViolation {
                severity,
                diagnostic,
            });
        }
    }

    /// Reports a finding on `net`, unless the net allows its rule.
    fn net_finding(
        &mut self,
        net: &Net,
        finding: NetFinding,
        pins: &[ErcPin],
        traces: &[Trace],
        no_connect: Option<&NodeMark>,
    ) {
        let name = net.name.clone();
        // The severity given by the pin map, for pin conflicts.
        let mut map_severity = None;
        let (rule, diagnostic) = match finding {
            NetFinding::PinToPin {
                pin,
                other,
                severity,
            } => {
                map_severity = Some(severity);
                let kind = ErcViolationKind::PinToPin {
                    net: name,
                    pin: pins[pin].clone(),
                    other: pins[other].clone(),
                };
                let note = format!("{} is here", pins[other]);
                (
                    ErcRule::PinToPin,
                    Diagnostic::new(kind, traces[pin].clone())
                        .with_note(note, traces[other].clone()),
                )
            }
            NetFinding::PinNotConnected { pin } => {
                let kind = ErcViolationKind::PinNotConnected {
                    net: name,
                    pin: pins[pin].clone(),
                };
                (
                    ErcRule::PinNotConnected,
                    Diagnostic::new(kind, traces[pin].clone()),
                )
            }
            NetFinding::PinNotDriven { pin } => {
                let kind = ErcViolationKind::PinNotDriven {
                    net: name,
                    pin: pins[pin].clone(),
                };
                (
                    ErcRule::PinNotDriven,
                    Diagnostic::new(kind, traces[pin].clone()),
                )
            }
            NetFinding::PowerPinNotDriven { pin } => {
                let kind = ErcViolationKind::PowerPinNotDriven {
                    net: name,
                    pin: pins[pin].clone(),
                };
                (
                    ErcRule::PowerPinNotDriven,
                    Diagnostic::new(kind, traces[pin].clone()),
                )
            }
            NetFinding::LabelSinglePin { pin } => {
                let kind = ErcViolationKind::LabelSinglePin {
                    net: name,
                    pin: pins[pin].clone(),
                };
                let note = format!("{} is here", pins[pin]);
                (
                    ErcRule::LabelSinglePin,
                    Diagnostic::new(kind, net.trace.clone()).with_note(note, traces[pin].clone()),
                )
            }
            NetFinding::NoConnectConnected { pins: connected } => {
                let kind = ErcViolationKind::NoConnectConnected {
                    net: name,
                    pins: connected.iter().map(|&pin| pins[pin].clone()).collect(),
                };
                let trace = no_connect
                    .map(|mark| mark.trace.clone())
                    .unwrap_or_default();
                let mut diagnostic = Diagnostic::new(kind, trace);
                for &pin in &connected {
                    diagnostic =
                        diagnostic.with_note(format!("{} is here", pins[pin]), traces[pin].clone());
                }
                (ErcRule::NoConnectConnected, diagnostic)
            }
        };
        if net.allows(rule) {
            return;
        }
        let severity = self.settings.severity(rule);
        self.push(
            map_severity.map_or(severity, |s| s.min(severity)),
            diagnostic,
        );
    }
}

/// Whether KiCad takes a net or pin name for a ground: it contains `GND`, or is `EARTH`
/// (`EARTH_...`), `VSS` or `VSSA`, ignoring case.
fn is_ground_name(name: &str) -> bool {
    let upper = name.to_uppercase();
    upper.contains("GND")
        || upper == "EARTH"
        || upper.starts_with("EARTH_")
        || upper == "VSS"
        || upper == "VSSA"
}

impl Net {
    /// Whether the net has an [`ErcMark::Allow`] mark for `rule`.
    fn allows(&self, rule: ErcRule) -> bool {
        self.marks
            .iter()
            .any(|mark| mark.mark == ErcMark::Allow(rule))
    }
}
