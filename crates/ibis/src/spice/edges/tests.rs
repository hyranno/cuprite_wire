use super::*;
use crate::VtPoint;

fn edges() -> Edges {
    Edges::linear(2.0, 1.0)
}

#[test]
fn test_weights_edges() {
    let w = edges().weights(
        Drive::Low,
        &[(10.0, Drive::High), (20.0, Drive::Low), (30.0, Drive::HiZ)],
    );
    assert_eq!(
        w.ku,
        [
            (0.0, 0.0),
            (10.0, 0.0),
            (12.0, 1.0),
            (20.0, 1.0),
            (21.0, 0.0)
        ]
    );
    // To HiZ from Low takes the rise time.
    assert_eq!(
        w.kd,
        [
            (0.0, 1.0),
            (10.0, 1.0),
            (12.0, 0.0),
            (20.0, 0.0),
            (21.0, 1.0),
            (30.0, 1.0),
            (32.0, 0.0)
        ]
    );
}

#[test]
fn test_weights_at_zero() {
    let w = edges().weights(Drive::HiZ, &[(0.0, Drive::High), (5.0, Drive::HiZ)]);
    assert_eq!(w.ku, [(0.0, 0.0), (2.0, 1.0), (5.0, 1.0), (6.0, 0.0)]);
    assert_eq!(w.kd, [(0.0, 0.0)]);
}

#[test]
fn test_weights_interrupted() {
    // Low at 11, halfway up: back down from 0.5 over half the fall time.
    let w = edges().weights(Drive::Low, &[(10.0, Drive::High), (11.0, Drive::Low)]);
    assert_eq!(w.ku, [(0.0, 0.0), (10.0, 0.0), (11.0, 0.5), (11.5, 0.0)]);
    assert_eq!(w.kd, [(0.0, 1.0), (10.0, 1.0), (11.0, 0.5), (11.5, 1.0)]);
    // An event repeating the state changes nothing.
    let w = edges().weights(Drive::High, &[(1.0, Drive::High), (2.0, Drive::HiZ)]);
    assert_eq!(w.ku, [(0.0, 1.0), (2.0, 1.0), (3.0, 0.0)]);
}

#[test]
#[should_panic(expected = "in order")]
fn test_weights_out_of_order() {
    edges().weights(Drive::Low, &[(2.0, Drive::High), (1.0, Drive::Low)]);
}

/// A buffer of resistors: 100 ohms up to 3.3 V and 50 ohms down to ground.
fn resistors() -> [IvCurve; 2] {
    let line = |r: f64, sign: f64| (vec![-5.0, 5.0], vec![-5.0 / r * sign, 5.0 / r * sign]);
    let (xs, ys) = line(100.0, -1.0);
    let pullup = IvCurve {
        id: "PU",
        xs,
        ys,
        reference: 3.3,
        from_reference: true,
        weight: Some("ku"),
    };
    let (xs, ys) = line(50.0, 1.0);
    let pulldown = IvCurve {
        id: "PD",
        xs,
        ys,
        reference: 0.0,
        from_reference: false,
        weight: Some("kd"),
    };
    [pullup, pulldown]
}

/// The weights of a rising edge over 1 ns after a delay of 0.2 ns: ku rises linearly, kd falls
/// twice as fast.
fn true_weights(t: f64) -> (f64, f64) {
    let x = ((t - 0.2e-9) / 1e-9).clamp(0.0, 1.0);
    (x, (1.0 - 2.0 * x).max(0.0))
}

/// The waveform of the resistors with the weights into `r_fixture` to `v_fixture`.
fn waveform(r_fixture: f64, v_fixture: f64) -> Waveform {
    let points = (0..=30)
        .map(|i| {
            let time = i as f64 * 0.05e-9;
            let (ku, kd) = true_weights(time);
            let conductance = ku / 100.0 + kd / 50.0 + 1.0 / r_fixture;
            let voltage = (ku * 3.3 / 100.0 + v_fixture / r_fixture) / conductance;
            VtPoint {
                time,
                voltage: crate::TypMinMax {
                    typ: voltage,
                    min: None,
                    max: None,
                },
            }
        })
        .collect();
    Waveform {
        r_fixture,
        v_fixture: crate::TypMinMax {
            typ: v_fixture,
            min: None,
            max: None,
        },
        l_fixture: None,
        c_fixture: Some(0.0),
        r_dut: None,
        l_dut: None,
        c_dut: None,
        points,
    }
}

#[test]
fn test_from_two_waveforms() {
    let [pullup, pulldown] = resistors();
    let buffer = Buffer {
        pullup: Some(&pullup),
        pulldown: Some(&pulldown),
        clamps: Vec::new(),
        c_comp: 0.0,
    };
    let mut notes = Vec::new();
    let mut unused = waveform(50.0, 1.0);
    unused.l_fixture = Some(1e-9);
    let waveforms = [waveform(50.0, 0.0), unused, waveform(50.0, 3.3)];
    let edge = from_waveforms(&buffer, &waveforms, true, Corner::Typ, &mut notes).unwrap();
    assert_eq!(
        notes,
        ["[Rising Waveform] 2 is not used: L_fixture 0.000000001 not supported"]
    );
    for t in [0.0, 0.3e-9, 0.6e-9, 0.9e-9, 1.5e-9] {
        let (ku, kd) = true_weights(t);
        assert!((value_at(&edge.ku, t) - ku).abs() < 2e-3, "ku at {t}");
        assert!((value_at(&edge.kd, t) - kd).abs() < 2e-3, "kd at {t}");
    }
    // Simplified: ku has its corners and its end only.
    assert_eq!(edge.ku.len(), 4, "{:?}", edge.ku);
}

#[test]
fn test_from_one_waveform() {
    let [pullup, pulldown] = resistors();
    // Open drain: kd only.
    let buffer = Buffer {
        pullup: None,
        pulldown: Some(&pulldown),
        clamps: Vec::new(),
        c_comp: 0.0,
    };
    let waveforms = [waveform(50.0, 3.3)];
    let edge = from_waveforms(&buffer, &waveforms, true, Corner::Typ, &mut Vec::new()).unwrap();
    // The waveform has ku: the open drain explains it with a smaller kd.
    assert!(value_at(&edge.kd, 0.1e-9) > 0.999);
    assert!(value_at(&edge.kd, 1.5e-9) < 1e-3);
    assert_eq!(edge.ku.last().unwrap().1, 1.0, "mirrored");

    // ku + kd = 1 with both tables.
    let buffer = Buffer {
        pullup: Some(&pullup),
        pulldown: Some(&pulldown),
        clamps: Vec::new(),
        c_comp: 0.0,
    };
    let edge = from_waveforms(&buffer, &waveforms, true, Corner::Typ, &mut Vec::new()).unwrap();
    for t in [0.0, 0.5e-9, 1.0e-9, 1.5e-9] {
        let sum = value_at(&edge.ku, t) + value_at(&edge.kd, t);
        assert!((sum - 1.0).abs() < 2e-3, "at {t}: {sum}");
    }
    assert!(from_waveforms(&buffer, &[], true, Corner::Typ, &mut Vec::new()).is_none());

    // A table that starts after the edge holds its first value from time 0; one that starts
    // before is cut at 0.
    for shift in [0.1e-9, -0.1e-9] {
        let mut w = waveform(50.0, 3.3);
        w.points.iter_mut().for_each(|p| p.time += shift);
        let edge = from_waveforms(&buffer, &[w], true, Corner::Typ, &mut Vec::new()).unwrap();
        assert_eq!(edge.ku[0], (0.0, 0.0));
        assert!(edge.ku.windows(2).all(|w| w[0].0 < w[1].0), "{:?}", edge.ku);
    }
}

#[test]
fn test_weights_follow_curves() {
    // A rising ku with a delay of 1 and a bend.
    let edge = |ku: Vec<(f64, f64)>| Edge {
        kd: ku.iter().map(|&(t, v)| (t, 1.0 - v)).collect(),
        ku,
    };
    let edges = Edges {
        rising: edge(vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.8), (3.0, 1.0)]),
        falling: edge(vec![(0.0, 1.0), (2.0, 0.0)]),
    };
    let w = edges.weights(Drive::Low, &[(10.0, Drive::High), (20.0, Drive::Low)]);
    assert_eq!(
        w.ku,
        [
            (0.0, 0.0),
            (10.0, 0.0),
            (11.0, 0.0),
            (12.0, 0.8),
            (13.0, 1.0),
            (20.0, 1.0),
            (22.0, 0.0)
        ]
    );
    // Cut at 0.4 on the way up: back down from the point of the falling curve at 0.4.
    let w = edges.weights(Drive::Low, &[(10.0, Drive::High), (11.5, Drive::Low)]);
    assert_eq!(
        w.ku,
        [
            (0.0, 0.0),
            (10.0, 0.0),
            (11.0, 0.0),
            (11.5, 0.4),
            (12.3, 0.0)
        ]
    );
    // And up again from 0.2: past the delay, along the bend.
    let w = edges.weights(
        Drive::Low,
        &[(10.0, Drive::High), (11.5, Drive::Low), (11.9, Drive::High)],
    );
    let last = &w.ku[w.ku.len() - 3..];
    assert!((last[0].0 - 11.9).abs() < 1e-12 && (last[0].1 - 0.2).abs() < 1e-12);
    assert!(
        (last[1].0 - 12.65).abs() < 1e-12 && last[1].1 == 0.8,
        "{last:?}"
    );
    assert_eq!(last[2].1, 1.0);
}
