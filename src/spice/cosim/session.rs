//! The state of a co-simulation run: the callbacks of the shared library dispatched to the models.

use std::collections::HashMap;

use super::{CosimDevice, CosimInstance, DeviceIo, DeviceModel, Drive, PinState};
use crate::spice::{
    GROUND,
    digital::{Level, OutputCharacteristics, Pwl},
    shared::Session,
};

/// The waveforms of the `EXTERNAL` sources of an output pin.
#[derive(Debug, Clone)]
struct OutputWaves {
    drive: Pwl,
    enable: Pwl,
}

impl OutputWaves {
    /// Outputs start `Low`, bidirectional pins `HiZ`.
    fn initial(output: &OutputCharacteristics, bidirectional: bool) -> Self {
        OutputWaves {
            drive: Pwl::new(output.low),
            enable: Pwl::new(if bidirectional { 0.0 } else { 1.0 }),
        }
    }

    /// Starts the change to `drive` at `time`; asks for time points at the ends of the edges.
    fn apply(
        &mut self,
        output: &OutputCharacteristics,
        time: f64,
        drive: Drive,
        breakpoint: &mut dyn FnMut(f64),
    ) {
        let (target, enable) = match drive {
            Drive::Level(Level::Low) => (Some(output.low), 1.0),
            Drive::Level(Level::High) => (Some(output.high), 1.0),
            Drive::Level(Level::HiZ) => (None, 0.0),
            Drive::Voltage(voltage) => (Some(voltage), 1.0),
        };
        if let Some(target) = target {
            self.drive
                .ramp_to(time, target, |from, to| output.edge_duration(from, to));
        }
        let transition = output.enable_transition();
        self.enable
            .ramp_to(time, enable, |from, to| transition * (to - from).abs());
        for pwl in [&mut self.drive, &mut self.enable] {
            if let Some(&(end, _)) = pwl.points.last().filter(|&&(end, _)| end > time) {
                breakpoint(end);
            }
            // Only the last ramp is evaluated.
            let excess = pwl.points.len().saturating_sub(2);
            pwl.points.drain(..excess);
        }
    }
}

struct DeviceRun<'m> {
    name: String,
    model: &'m mut dyn DeviceModel,
    pins: Vec<PinState>,
    /// For each pin with output characteristics.
    waves: Vec<Option<OutputWaves>>,
    /// The nodes of the pins followed by `gnd`, in lowercase as ngspice names the vectors.
    nodes: Vec<String>,
    /// The index of the vector of each node in the current plot (`None` for the ground).
    vectors: Vec<Option<usize>>,
    /// Whether an accepted point of the current plot has been seen.
    started: bool,
}

impl DeviceRun<'_> {
    fn reset(&mut self) {
        for (pin, waves) in self.pins.iter_mut().zip(&mut self.waves) {
            *waves = pin
                .pin
                .output
                .as_ref()
                .map(|output| OutputWaves::initial(output, pin.pin.input.is_some()));
            pin.voltage = 0.0;
            pin.read = false;
            pin.request = None;
        }
        self.started = false;
    }
}

/// Which source of an output pin.
#[derive(Debug, Clone, Copy)]
enum Source {
    Drive,
    Enable,
}

/// The models of a run and the state of their devices.
pub(crate) struct CosimSession<'m> {
    devices: Vec<DeviceRun<'m>>,
    /// `EXTERNAL` source name -> (device, pin, source).
    sources: HashMap<String, (usize, usize, Source)>,
    /// The index of the `time` vector, in a transient analysis.
    time: Option<usize>,
    wakes: Vec<f64>,
}

impl<'m> CosimSession<'m> {
    /// Binds the models to the devices of a netlist by name.
    pub(crate) fn new(
        instances: &[CosimInstance],
        models: Vec<(String, &'m mut dyn DeviceModel)>,
    ) -> Result<Self, String> {
        let mut models: Vec<Option<(String, &'m mut dyn DeviceModel)>> =
            models.into_iter().map(Some).collect();
        let mut devices = Vec::new();
        let mut sources = HashMap::new();
        for (index, instance) in instances.iter().enumerate() {
            let name = instance.device.name();
            if let Some(other) = instances[..index]
                .iter()
                .find(|other| other.device.name() == name)
            {
                return Err(format!(
                    "the device {name:?} is in the circuit more than once ({} and {})",
                    other.element, instance.element
                ));
            }
            let model = models
                .iter_mut()
                .find(|entry| entry.as_ref().is_some_and(|(n, _)| n == name))
                .and_then(Option::take)
                .map(|(_, model)| model)
                .ok_or_else(|| format!("no model is given for the device {name:?}"))?;
            let pins: Vec<PinState> = instance
                .device
                .pins()
                .iter()
                .map(|pin| PinState {
                    pin: pin.clone(),
                    voltage: 0.0,
                    read: false,
                    request: None,
                })
                .collect();
            for (pin_index, pin) in pins.iter().enumerate() {
                if pin.pin.output.is_some() {
                    for (kind, source) in [('d', Source::Drive), ('e', Source::Enable)] {
                        sources.insert(
                            CosimDevice::source_name(&instance.element, kind, &pin.pin.name),
                            (devices.len(), pin_index, source),
                        );
                    }
                }
            }
            let mut device = DeviceRun {
                name: name.to_string(),
                model,
                waves: vec![None; pins.len()],
                pins,
                nodes: instance
                    .nodes
                    .iter()
                    .map(|node| node.to_ascii_lowercase())
                    .collect(),
                vectors: Vec::new(),
                started: false,
            };
            device.reset();
            devices.push(device);
        }
        if let Some((name, _)) = models.into_iter().flatten().next() {
            return Err(format!("the circuit has no device {name:?} for a model"));
        }
        Ok(CosimSession {
            devices,
            sources,
            time: None,
            wakes: Vec::new(),
        })
    }
}

impl Session for CosimSession<'_> {
    fn begin_plot(&mut self, plot_type: &str, vectors: &[String]) {
        let transient = plot_type.to_ascii_lowercase().starts_with("tran");
        let index: HashMap<&str, usize> = vectors
            .iter()
            .enumerate()
            .map(|(i, name)| (name.as_str(), i))
            .collect();
        self.time = index.get("time").copied().filter(|_| transient);
        for device in &mut self.devices {
            device.vectors = device
                .nodes
                .iter()
                .map(|node| {
                    (node != GROUND)
                        .then(|| index.get(node.as_str()).copied())
                        .flatten()
                })
                .collect();
            if transient {
                device.reset();
            }
        }
    }

    fn accepted(&mut self, values: &[f64], breakpoint: &mut dyn FnMut(f64)) {
        let Some(time) = self.time.and_then(|i| values.get(i).copied()) else {
            return;
        };
        for device in &mut self.devices {
            let voltage = |vector: Option<usize>| vector.and_then(|i| values.get(i)).copied();
            let gnd = voltage(*device.vectors.last().expect("gnd is a node")).unwrap_or(0.0);
            for (pin, &vector) in device.pins.iter_mut().zip(&device.vectors) {
                pin.voltage = voltage(vector).unwrap_or(0.0) - gnd;
                if let Some(input) = &pin.pin.input {
                    let previous = if device.started {
                        pin.read
                    } else {
                        pin.voltage >= (input.threshold_low + input.threshold_high) / 2.0
                    };
                    pin.read = input.read(pin.voltage, previous);
                }
                pin.request = None;
            }
            device.started = true;
            self.wakes.clear();
            device.model.step(&mut DeviceIo {
                time,
                pins: &mut device.pins,
                wakes: &mut self.wakes,
            });
            for (pin, waves) in device.pins.iter_mut().zip(&mut device.waves) {
                if let (Some(drive), Some(output), Some(waves)) =
                    (pin.request.take(), &pin.pin.output, waves.as_mut())
                {
                    waves.apply(output, time, drive, breakpoint);
                }
            }
            for &wake in &self.wakes {
                breakpoint(wake);
            }
        }
    }

    fn source(&mut self, name: &str, time: f64) -> f64 {
        let Some(&(device, pin, source)) = self.sources.get(name) else {
            return 0.0;
        };
        let device = &self.devices[device];
        let Some(waves) = &device.waves[pin] else {
            return 0.0;
        };
        match source {
            Source::Drive => waves.drive.value_at(time),
            Source::Enable => waves.enable.value_at(time),
        }
    }
}

impl std::fmt::Debug for CosimSession<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.devices.iter().map(|d| d.name.as_str()).collect();
        f.debug_struct("CosimSession")
            .field("devices", &names)
            .finish_non_exhaustive()
    }
}
