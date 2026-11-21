//! Electrical rules check (ERC) of a netlist, following the netlist-level rules of KiCad 9's ERC.
//!
//! The electrical types of pins come from [`Part::pin_types`](crate::parts::Part::pin_types).
//! Marks attached to nodes stand for the schematic items KiCad uses: a no-connect flag
//! ([`CircuitBuilder::no_connect`]), a `PWR_FLAG` ([`CircuitBuilder::power_flag`]), and pins outside the
//! circuit ([`CircuitBuilder::virtual_pin`]). [`check_subcircuit`] checks a subcircuit alone.
//!
//! See the guide [Electrical rules check](https://github.com/hyranno/cuprite_wire/blob/main/doc/erc.md).
//!
//! [`CircuitBuilder::no_connect`]: crate::circuit::CircuitBuilder::no_connect
//! [`CircuitBuilder::power_flag`]: crate::circuit::CircuitBuilder::power_flag
//! [`CircuitBuilder::virtual_pin`]: crate::circuit::CircuitBuilder::virtual_pin

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
use rules::{NetFinding, NetPin, check_net};

/// A mark attached to a node for ERC.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// KiCad 9's default (`erc.pin_map` of a new project).
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

/// The rules other than pin conflicts, whose severities are given by [`PinMap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ErcRule {
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
}

impl ErcRule {
    /// The default severity, KiCad's except for [`ErcRule::LabelOnlyNet`] (an error in KiCad).
    pub fn default_severity(self) -> Severity {
        match self {
            ErcRule::PinNotConnected | ErcRule::PinNotDriven | ErcRule::PowerPinNotDriven => {
                Severity::Error
            }
            ErcRule::NoConnectConnected
            | ErcRule::NoConnectDangling
            | ErcRule::VirtualPinDangling
            | ErcRule::SimilarNetNames
            | ErcRule::MultipleNetNames
            | ErcRule::LabelOnlyNet => Severity::Warning,
        }
    }
}

/// The severities ERC uses; the default is KiCad 9's (see [`ErcRule::default_severity`]).
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

/// What an ERC violation is about. Each variant but [`ErcViolationKind::PinToPin`] corresponds to
/// the [`ErcRule`] of the same name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErcViolationKind {
    /// Two pins whose types conflict in the [`PinMap`]. Traced to `pin`, with a note at `other`.
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
    /// A warning of the netlist conversion.
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
    /// and [`ErcRule::LabelOnlyNet`]).
    pub fn erc(&self, settings: &ErcSettings) -> ErcReport {
        let mut report = Reporter {
            settings,
            violations: Vec::new(),
        };
        self.check_nets(&mut report);
        self.check_pinless_marks(&mut report);
        self.check_similar_net_names(&mut report);
        self.report_warnings(&mut report);
        ErcReport {
            violations: report.violations,
        }
    }

    /// Checks the pins and marks on each net.
    fn check_nets(&self, report: &mut Reporter) {
        let pin_types: Vec<_> = self.components.iter().map(|c| c.part.pin_types()).collect();
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
            let (pins, net_pins, traces) = self.net_pins(net, &pin_types, &stack_groups);
            let no_connect = net
                .marks
                .iter()
                .find(|mark| mark.mark == ErcMark::NoConnect);
            let pin_map = &report.settings.pin_map;
            for finding in check_net(&net_pins, no_connect.is_some(), pin_map) {
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

    /// Reports the marks on nets without part pins.
    fn check_pinless_marks(&self, report: &mut Reporter) {
        for mark in &self.pinless_marks {
            let (rule, kind) = match &mark.mark {
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
            let rule = match warning.kind {
                CircuitWarningKind::MultipleNetNames { .. } => ErcRule::MultipleNetNames,
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

    fn net_finding(
        &mut self,
        net: &Net,
        finding: NetFinding,
        pins: &[ErcPin],
        traces: &[Trace],
        no_connect: Option<&NodeMark>,
    ) {
        let name = net.name.clone();
        let (severity, diagnostic) = match finding {
            NetFinding::PinToPin {
                pin,
                other,
                severity,
            } => {
                let kind = ErcViolationKind::PinToPin {
                    net: name,
                    pin: pins[pin].clone(),
                    other: pins[other].clone(),
                };
                let note = format!("{} is here", pins[other]);
                (
                    severity,
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
                    self.settings.severity(ErcRule::PinNotConnected),
                    Diagnostic::new(kind, traces[pin].clone()),
                )
            }
            NetFinding::PinNotDriven { pin } => {
                let kind = ErcViolationKind::PinNotDriven {
                    net: name,
                    pin: pins[pin].clone(),
                };
                (
                    self.settings.severity(ErcRule::PinNotDriven),
                    Diagnostic::new(kind, traces[pin].clone()),
                )
            }
            NetFinding::PowerPinNotDriven { pin } => {
                let kind = ErcViolationKind::PowerPinNotDriven {
                    net: name,
                    pin: pins[pin].clone(),
                };
                (
                    self.settings.severity(ErcRule::PowerPinNotDriven),
                    Diagnostic::new(kind, traces[pin].clone()),
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
                (
                    self.settings.severity(ErcRule::NoConnectConnected),
                    diagnostic,
                )
            }
        };
        self.push(severity, diagnostic);
    }
}
