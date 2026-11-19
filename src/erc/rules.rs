//! The checks made on each net, as pure functions of the electrical types of its pins.
//!
//! They follow KiCad 9's `ERC_TESTER::TestPinToPin` (`eeschema/erc/erc.cpp`) and
//! `CONNECTION_GRAPH::ercCheckNoConnects` (`eeschema/connection_graph.cpp`).

use std::collections::HashSet;

use super::{PinMap, Severity};
use crate::parts::PinType;

/// A pin of a net, as seen by the checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NetPin {
    pub pin_type: PinType,
    /// Pins of a part with the same key are connected inside the part (like KiCad's stacked pins):
    /// a conflict between them is not reported, and they count as one pin. `None` for virtual pins.
    pub stack: Option<(usize, usize)>,
}

impl NetPin {
    fn is_virtual(&self) -> bool {
        self.stack.is_none()
    }
}

/// A problem found on one net. Pins are indices into the pins given to [`check_net`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NetFinding {
    PinToPin {
        pin: usize,
        other: usize,
        severity: Severity,
    },
    PinNotConnected {
        pin: usize,
    },
    PinNotDriven {
        pin: usize,
    },
    PowerPinNotDriven {
        pin: usize,
    },
    NoConnectConnected {
        pins: Vec<usize>,
    },
}

/// Pin types driving a net without `power_in` pins.
const DRIVING: [PinType; 5] = [
    PinType::Output,
    PinType::PowerOut,
    PinType::Passive,
    PinType::TriState,
    PinType::Bidirectional,
];

/// Pin types needing a driver on their net.
const DRIVEN: [PinType; 2] = [PinType::Input, PinType::PowerIn];

/// The order in which KiCad picks the pin a conflict is reported on (descending).
fn report_weight(pin_type: PinType) -> u32 {
    match pin_type {
        PinType::Free => 11,
        PinType::Unspecified => 10,
        PinType::Passive => 9,
        PinType::OpenCollector => 8,
        PinType::OpenEmitter => 7,
        PinType::Input => 6,
        PinType::TriState => 5,
        PinType::Bidirectional => 4,
        PinType::Output => 3,
        PinType::PowerIn => 2,
        PinType::PowerOut => 1,
        PinType::NoConnect => 0,
    }
}

fn is_stacked(a: &NetPin, b: &NetPin) -> bool {
    a.stack.is_some() && a.stack == b.stack && a.pin_type == b.pin_type
}

/// Checks one net. `pins` are its part pins followed by its virtual pins, and `no_connect` tells
/// whether the net has a no-connect mark. Findings are in a deterministic order.
pub(crate) fn check_net(pins: &[NetPin], no_connect: bool, pin_map: &PinMap) -> Vec<NetFinding> {
    let mut findings = pin_to_pin(pins, pin_map);

    // Pins needing a driver. A net with a `power_in` pin is a power net, which only `power_out`
    // pins drive; on it, a `power_in` pin is reported rather than an input pin.
    let power_net = pins.iter().any(|pin| pin.pin_type == PinType::PowerIn);
    let needs_driver = pins
        .iter()
        .position(|pin| power_net && pin.pin_type == PinType::PowerIn)
        .or_else(|| pins.iter().position(|pin| DRIVEN.contains(&pin.pin_type)));
    let has_driver = pins.iter().any(|pin| {
        if power_net {
            pin.pin_type == PinType::PowerOut
        } else {
            DRIVING.contains(&pin.pin_type)
        }
    });
    if let Some(pin) = needs_driver
        && !has_driver
        && !no_connect
    {
        findings.push(if power_net {
            NetFinding::PowerPinNotDriven { pin }
        } else {
            NetFinding::PinNotDriven { pin }
        });
    }

    // Pins of parts, counting connected pins of one part once.
    let mut seen = HashSet::new();
    let unique: Vec<usize> = (0..pins.len())
        .filter(|&i| !pins[i].is_virtual() && seen.insert((pins[i].stack, pins[i].pin_type)))
        .collect();
    if no_connect {
        if unique.len() > 1 {
            findings.push(NetFinding::NoConnectConnected { pins: unique });
        }
    } else if unique.len() == 1 && !pins.iter().any(NetPin::is_virtual) {
        let pin = unique[0];
        if !matches!(pins[pin].pin_type, PinType::NoConnect | PinType::Free) {
            findings.push(NetFinding::PinNotConnected { pin });
        }
    }
    findings
}

/// Conflicting pairs of pin types. Like KiCad, each pin with a conflict is reported once, starting
/// from the pins with the larger [`report_weight`], and the conflicts reported with a pin are not
/// reported again. KiCad pairs the pin with the nearest conflicting pin on the sheet; here it is
/// paired with the conflicting pin of the highest severity (the first one among those).
fn pin_to_pin(pins: &[NetPin], pin_map: &PinMap) -> Vec<NetFinding> {
    let mut conflicts = Vec::new();
    for (i, a) in pins.iter().enumerate() {
        for (j, b) in pins.iter().enumerate().skip(i + 1) {
            let severity = pin_map.get(a.pin_type, b.pin_type);
            if severity != Severity::Ignore && !is_stacked(a, b) {
                conflicts.push((i, j, severity));
            }
        }
    }
    let mut order: Vec<usize> = (0..pins.len())
        .filter(|&pin| conflicts.iter().any(|&(i, j, _)| i == pin || j == pin))
        .collect();
    // A stable sort keeps the pin order among the pins of the same weight.
    order.sort_by_key(|&pin| std::cmp::Reverse(report_weight(pins[pin].pin_type)));

    let mut findings = Vec::new();
    for pin in order {
        let (involved, rest): (Vec<_>, Vec<_>) = conflicts
            .into_iter()
            .partition(|&(i, j, _)| i == pin || j == pin);
        conflicts = rest;
        let partner = involved
            .into_iter()
            .map(|(i, j, severity)| (if i == pin { j } else { i }, severity))
            .min_by_key(|&(other, severity)| (std::cmp::Reverse(severity), other));
        if let Some((other, severity)) = partner {
            findings.push(NetFinding::PinToPin {
                pin,
                other,
                severity,
            });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use PinType::*;

    /// Pins of different parts.
    fn pins(types: &[PinType]) -> Vec<NetPin> {
        types
            .iter()
            .enumerate()
            .map(|(i, &pin_type)| NetPin {
                pin_type,
                stack: Some((i, 0)),
            })
            .collect()
    }

    fn virtual_pin(pin_type: PinType) -> NetPin {
        NetPin {
            pin_type,
            stack: None,
        }
    }

    fn check(pins: &[NetPin]) -> Vec<NetFinding> {
        check_net(pins, false, &PinMap::default())
    }

    #[test]
    fn test_pin_map_is_kicads_default() {
        // erc.pin_map of a KiCad 9 project, in the order of PinType::ALL.
        let kicad = [
            [0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 2],
            [0, 2, 0, 1, 0, 0, 1, 0, 2, 2, 2, 2],
            [0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 1, 2],
            [0, 1, 0, 0, 0, 0, 1, 1, 2, 1, 1, 2],
            [0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 2],
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2],
            [1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 2],
            [0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 2],
            [0, 2, 1, 2, 0, 0, 1, 0, 2, 2, 2, 2],
            [0, 2, 0, 1, 0, 0, 1, 0, 2, 0, 0, 2],
            [0, 2, 1, 1, 0, 0, 1, 0, 2, 0, 0, 2],
            [2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2],
        ];
        let pin_map = PinMap::default();
        for a in PinType::ALL {
            for b in PinType::ALL {
                let expected = [Severity::Ignore, Severity::Warning, Severity::Error]
                    [kicad[a.index()][b.index()]];
                assert_eq!(pin_map.get(a, b), expected, "{a} - {b}");
                // Every cell is checked against a pin pair on a net.
                let conflicts: Vec<_> = check(&pins(&[a, b]))
                    .into_iter()
                    .filter_map(|finding| match finding {
                        NetFinding::PinToPin { severity, .. } => Some(severity),
                        _ => None,
                    })
                    .collect();
                let expected_conflicts = if expected == Severity::Ignore {
                    vec![]
                } else {
                    vec![expected]
                };
                assert_eq!(conflicts, expected_conflicts, "{a} - {b}");
            }
        }
    }

    #[test]
    fn test_conflicts_are_reported_once_per_pin() {
        // The unspecified pin (largest weight) is reported first with all its conflicts;
        // the two outputs are then reported together.
        assert_eq!(
            check(&pins(&[Output, Unspecified, Output])),
            vec![
                NetFinding::PinToPin {
                    pin: 1,
                    other: 0,
                    severity: Severity::Warning
                },
                NetFinding::PinToPin {
                    pin: 0,
                    other: 2,
                    severity: Severity::Error
                },
            ]
        );
        // The most severe conflict of a pin is reported.
        assert_eq!(
            check(&pins(&[TriState, PowerOut, Output])),
            vec![
                NetFinding::PinToPin {
                    pin: 0,
                    other: 1,
                    severity: Severity::Error
                },
                NetFinding::PinToPin {
                    pin: 2,
                    other: 1,
                    severity: Severity::Error
                },
            ]
        );
    }

    #[test]
    fn test_stacked_pins_do_not_conflict() {
        let stacked = [
            NetPin {
                pin_type: Output,
                stack: Some((0, 0)),
            },
            NetPin {
                pin_type: Output,
                stack: Some((0, 0)),
            },
            NetPin {
                pin_type: Input,
                stack: Some((1, 0)),
            },
        ];
        assert_eq!(check(&stacked), vec![]);
        // Pins of one part that are not connected inside it conflict, and count as two pins.
        let separate = [
            NetPin {
                pin_type: Output,
                stack: Some((0, 0)),
            },
            NetPin {
                pin_type: Output,
                stack: Some((0, 1)),
            },
        ];
        assert_eq!(
            check(&separate),
            vec![NetFinding::PinToPin {
                pin: 0,
                other: 1,
                severity: Severity::Error
            }]
        );
        // Stacked pins alone are not connected to anything else.
        assert_eq!(
            check(&stacked[..2]),
            vec![NetFinding::PinNotConnected { pin: 0 }]
        );
    }

    #[test]
    fn test_drivers() {
        for driver in [Output, PowerOut, Passive, TriState, Bidirectional] {
            assert_eq!(check(&pins(&[Input, driver])), vec![], "{driver}");
        }
        for other in [Input, Unspecified, OpenCollector, OpenEmitter, Free] {
            assert!(
                check(&pins(&[Input, other])).contains(&NetFinding::PinNotDriven { pin: 0 }),
                "{other}"
            );
        }
        // Only power_out drives a net with a power_in pin, and the power_in pin is reported.
        assert_eq!(
            check(&pins(&[Input, PowerIn, Passive, Output])),
            vec![NetFinding::PowerPinNotDriven { pin: 1 }]
        );
        assert_eq!(check(&pins(&[Input, PowerIn, PowerOut])), vec![]);
        // A virtual pin drives the net.
        assert_eq!(check(&[pins(&[PowerIn])[0], virtual_pin(PowerOut)]), vec![]);
        // Nothing to drive.
        assert_eq!(check(&pins(&[Passive, Output])), vec![]);
    }

    #[test]
    fn test_unconnected_pins() {
        assert_eq!(
            check(&pins(&[Passive])),
            vec![NetFinding::PinNotConnected { pin: 0 }]
        );
        // KiCad reports both for a lone input pin.
        assert_eq!(
            check(&pins(&[Input])),
            vec![
                NetFinding::PinNotDriven { pin: 0 },
                NetFinding::PinNotConnected { pin: 0 }
            ]
        );
        assert_eq!(check(&pins(&[NoConnect])), vec![]);
        assert_eq!(check(&pins(&[Free])), vec![]);
        // A virtual pin is something to connect to.
        assert_eq!(check(&[pins(&[Passive])[0], virtual_pin(Output)]), vec![]);
    }

    #[test]
    fn test_no_connect_mark() {
        let pin_map = PinMap::default();
        // The mark silences unconnected and undriven pins.
        assert_eq!(check_net(&pins(&[Input]), true, &pin_map), vec![]);
        assert_eq!(
            check_net(&pins(&[Input, Passive]), true, &pin_map),
            vec![NetFinding::NoConnectConnected { pins: vec![0, 1] }]
        );
        // Virtual pins do not count as connections.
        assert_eq!(
            check_net(&[pins(&[Passive])[0], virtual_pin(Output)], true, &pin_map),
            vec![]
        );
    }
}
