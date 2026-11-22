//! Running decks in the ngspice shared library (libngspice), with the `shared-ngspice` feature.
//!
//! The library is loaded at run time, so building does not need it. It keeps global state, so it
//! is loaded once per process and runs one deck at a time: runs from several threads (the tests of
//! `cargo test`) wait for each other. A crash of ngspice takes the whole process down, which the
//! CLI ([`Ngspice`](crate::spice::Ngspice)) does not; prefer the CLI unless the run needs the
//! library (co-simulation with Rust models).

use std::{
    any::Any,
    cell::RefCell,
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs, io,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    path::{Path, PathBuf},
    ptr,
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
};

use libloading::Library;

use crate::spice::{
    SpiceDeck,
    simulation::{Simulation, SimulationError, Simulator, finish_run, prepare_run_dir},
    spice_path,
};

/// The deck as the library runs it, next to [`DECK_FILE`](crate::spice::simulation::DECK_FILE).
const SHARED_DECK_FILE: &str = "shared.cir";

/// The file name of the library, found in the search path of the dynamic loader.
const DEFAULT_LIBRARY: &str = if cfg!(windows) {
    "ngspice.dll"
} else if cfg!(target_os = "macos") {
    "libngspice.0.dylib"
} else {
    "libngspice.so.0"
};

/// The ngspice shared library.
///
/// It runs the same deck as [`Ngspice`](crate::spice::Ngspice) and gives the same [`Simulation`]:
/// the run directory holds `deck.cir` (`ngspice -b deck.cir` there reproduces the run with the CLI)
/// and `shared.cir`, the form the library runs. That form has no `quit`, and the relative output
/// files of `wrdata`, `write` and `wrs2p` are made absolute, since the library runs in the working
/// directory of the process (`write` without a file writes `rawspice.raw` in the run directory, as
/// the CLI does). Output files of other commands (e.g. `print ... > file`) must be absolute, and
/// `wrs2p` needs a file. The compatibility mode is set with `set ngbehavior=...` before the deck is
/// read. A deck without a control block is run with `run` after it is read, as the CLI runs the
/// analyses of its dot directives.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SharedNgspice {
    /// The library file: a path, or a file name the dynamic loader searches for.
    pub library: PathBuf,
}

impl Default for SharedNgspice {
    /// `libngspice.so.0` (`libngspice.0.dylib` on macOS, `ngspice.dll` on Windows).
    fn default() -> Self {
        SharedNgspice {
            library: DEFAULT_LIBRARY.into(),
        }
    }
}

impl SharedNgspice {
    /// Uses the library `library` (a path, or a file name the dynamic loader searches for).
    pub fn new(library: impl Into<PathBuf>) -> Self {
        SharedNgspice {
            library: library.into(),
        }
    }

    /// Runs the deck in a new temporary directory.
    ///
    /// After an error ngspice cannot recover from, the library is unloaded and loaded again for
    /// the next run.
    ///
    /// # Errors
    /// [`SimulationError::SharedLibrary`] if the library cannot be loaded or used,
    /// [`SimulationError::Ngspice`] if ngspice reports an error, and [`SimulationError::Io`] if
    /// the run directory cannot be prepared.
    pub fn run(&self, deck: &SpiceDeck) -> Result<Simulation, SimulationError> {
        self.run_session(deck, None)
    }

    /// [`SharedNgspice::run`], passing the data of the run to `session` as it progresses.
    /// A panic in `session` is resumed after the run.
    pub(crate) fn run_session(
        &self,
        deck: &SpiceDeck,
        session: Option<&mut dyn Session>,
    ) -> Result<Simulation, SimulationError> {
        let dir = prepare_run_dir(deck)?;
        // ngspice takes the rest of a command line as it is, quotes included.
        let deck_path = dir.join(SHARED_DECK_FILE);
        if deck_path
            .to_str()
            .is_none_or(|path| path.contains(|c: char| c.is_whitespace() || c == '"'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the run directory {} cannot be written in a command of ngspice",
                    dir.display()
                ),
            )
            .into());
        }
        fs::write(&deck_path, deck.for_shared_library(&dir))?;

        let mut library = lock(&LIBRARY);
        if library.as_ref().is_none_or(|l| l.path != self.library) {
            // Unload the other library first: both would share the callbacks.
            *library = None;
            *library = Some(Loaded::load(&self.library)?);
        }
        let loaded = library.as_ref().expect("the library is loaded");
        lock(&OUTPUT).clear();
        EXITED.store(false, Ordering::SeqCst);
        let mode = match deck.netlist().ngbehavior() {
            Some(mode) => format!("set ngbehavior={mode}"),
            None => "unset ngbehavior".into(),
        };
        let source = format!("source {}", spice_path(&deck_path));
        // Without a control block the CLI runs the analyses of the dot directives; `source` only
        // loads the circuit.
        let commands = [mode.as_str(), source.as_str()]
            .into_iter()
            .chain((!deck.has_control()).then_some("run"));
        let status = {
            let _active = session.map(|session| Active::start(session, loaded.set_breakpoint));
            commands
                .map(|command| loaded.command(command))
                .find(|status| !matches!(status, Ok(0)) || EXITED.load(Ordering::SeqCst))
                .unwrap_or(Ok(0))
        };
        let panic = Active::take_panic();
        let log = std::mem::take(&mut *lock(&OUTPUT));
        let exited = EXITED.load(Ordering::SeqCst);
        if exited {
            // "ngspice.dll cannot recover and awaits to be reset or detached".
            *library = None;
        } else {
            // Free the circuit and its results.
            for command in ["remcirc", "destroy all"] {
                loaded.command(command).expect("no NUL character");
            }
            lock(&OUTPUT).clear();
        }
        drop(library);
        if let Some(panic) = panic {
            resume_unwind(panic);
        }
        let succeeded = status? == 0;
        finish_run(dir, log, succeeded && !exited)
    }
}

/// Receives the data of a run as it progresses: the callbacks of the shared library, called in
/// the thread running the deck.
pub(crate) trait Session {
    /// A plot (the results of an analysis, e.g. `tran1`, `op1`) starts. `vectors` are the names of
    /// its vectors (node voltages are named by the node, in lowercase: `/out`, `xu1.d`), in the
    /// order of the values passed to [`Session::accepted`].
    fn begin_plot(&mut self, plot_type: &str, vectors: &[String]);
    /// A point of the plot has been accepted. `values` are the real parts of the vectors.
    /// `breakpoint` asks ngspice to put a time point at the given time.
    fn accepted(&mut self, values: &[f64], breakpoint: &mut dyn FnMut(f64));
    /// The value of the `EXTERNAL` voltage source `name` (in lowercase, with the path of its
    /// subcircuit instance: `v.xu1.vd_pa0`) at `time`, which may be a trial time point that is
    /// rejected later.
    fn source(&mut self, name: &str, time: f64) -> f64;
}

/// The session of the run in progress in this thread.
struct Active {
    /// Valid until the [`ActiveGuard`] that set it is dropped.
    session: *mut (dyn Session + 'static),
    set_breakpoint: NgSpiceSetBkpt,
    /// The type of the plot announced by `SendInitData`, until its first point.
    pending_plot: Option<String>,
    values: Vec<f64>,
    panic: Option<Box<dyn Any + Send>>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
    /// The panic of the session of the last run, kept after its guard is dropped.
    static PANIC: RefCell<Option<Box<dyn Any + Send>>> = const { RefCell::new(None) };
}

/// Removes the session when dropped.
struct ActiveGuard;

impl Active {
    fn start(session: &mut dyn Session, set_breakpoint: NgSpiceSetBkpt) -> ActiveGuard {
        let session: *mut (dyn Session + '_) = session;
        // SAFETY: only the lifetime is erased; the guard removes the pointer before it ends.
        let session: *mut (dyn Session + 'static) = unsafe { std::mem::transmute(session) };
        ACTIVE.with_borrow_mut(|active| {
            *active = Some(Active {
                session,
                set_breakpoint,
                pending_plot: None,
                values: Vec::new(),
                panic: None,
            })
        });
        ActiveGuard
    }

    /// The panic of the session that has just ended, if any.
    fn take_panic() -> Option<Box<dyn Any + Send>> {
        PANIC.with_borrow_mut(Option::take)
    }

    /// Calls `f` with the session, unless there is none in this thread or it has panicked.
    fn with<R>(f: impl FnOnce(&mut Active, &mut dyn Session) -> R) -> Option<R> {
        ACTIVE.with_borrow_mut(|active| {
            let active = active.as_mut().filter(|active| active.panic.is_none())?;
            // SAFETY: the session outlives the guard, and is used by this thread only.
            let session = unsafe { &mut *active.session };
            match catch_unwind(AssertUnwindSafe(|| f(active, session))) {
                Ok(result) => Some(result),
                Err(panic) => {
                    active.panic = Some(panic);
                    None
                }
            }
        })
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let panic = ACTIVE.with_borrow_mut(|active| active.take().and_then(|a| a.panic));
        PANIC.with_borrow_mut(|slot| *slot = panic);
    }
}

impl Simulator for SharedNgspice {
    fn run(&self, deck: &SpiceDeck) -> Result<Simulation, SimulationError> {
        SharedNgspice::run(self, deck)
    }
}

/// The loaded library; `None` before the first run and after an error ngspice cannot recover from.
static LIBRARY: Mutex<Option<Loaded>> = Mutex::new(None);
/// The output of ngspice since it was last cleared, one line per call of [`send_char`].
static OUTPUT: Mutex<String> = Mutex::new(String::new());
/// Set by [`controlled_exit`].
static EXITED: AtomicBool = AtomicBool::new(false);

/// A run that panicked leaves nothing the next run relies on, so a poisoned lock is used as it is.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// The callbacks and functions of `sharedspice.h` used here.
type SendChar = unsafe extern "C" fn(*mut c_char, c_int, *mut c_void) -> c_int;
type SendStat = unsafe extern "C" fn(*mut c_char, c_int, *mut c_void) -> c_int;
type ControlledExit = unsafe extern "C" fn(c_int, bool, bool, c_int, *mut c_void) -> c_int;
type BgThreadRunning = unsafe extern "C" fn(bool, c_int, *mut c_void) -> c_int;
type SendData = unsafe extern "C" fn(*mut VecValuesAll, c_int, c_int, *mut c_void) -> c_int;
type SendInitData = unsafe extern "C" fn(*mut VecInfoAll, c_int, *mut c_void) -> c_int;
type GetVsrcData = unsafe extern "C" fn(*mut f64, f64, *mut c_char, c_int, *mut c_void) -> c_int;
type NgSpiceInit = unsafe extern "C" fn(
    Option<SendChar>,
    Option<SendStat>,
    Option<ControlledExit>,
    Option<SendData>,
    Option<SendInitData>,
    Option<BgThreadRunning>,
    *mut c_void,
) -> c_int;
/// `ngSpice_Init_Sync(GetVSRCData*, GetISRCData*, GetSyncData*, int*, void*)`; the current
/// sources and the time step control are not used (null).
type NgSpiceInitSync = unsafe extern "C" fn(
    Option<GetVsrcData>,
    *mut c_void,
    *mut c_void,
    *mut c_int,
    *mut c_void,
) -> c_int;
type NgSpiceCommand = unsafe extern "C" fn(*mut c_char) -> c_int;
type NgSpiceSetBkpt = unsafe extern "C" fn(f64) -> bool;

#[repr(C)]
struct VecValues {
    name: *mut c_char,
    creal: f64,
    cimag: f64,
    is_scale: bool,
    is_complex: bool,
}

#[repr(C)]
struct VecValuesAll {
    veccount: c_int,
    vecindex: c_int,
    vecsa: *mut *mut VecValues,
}

#[repr(C)]
struct VecInfo {
    number: c_int,
    vecname: *mut c_char,
    is_real: bool,
    pdvec: *mut c_void,
    pdvecscale: *mut c_void,
}

#[repr(C)]
struct VecInfoAll {
    name: *mut c_char,
    title: *mut c_char,
    date: *mut c_char,
    type_: *mut c_char,
    veccount: c_int,
    vecs: *mut *mut VecInfo,
}

/// The identification number ngspice gives the library (`ngSpice_Init_Sync` keeps the pointer).
static IDENT: AtomicI32 = AtomicI32::new(0);

struct Loaded {
    path: PathBuf,
    command: NgSpiceCommand,
    set_breakpoint: NgSpiceSetBkpt,
    /// Keeps `command` valid; unloaded on drop.
    _library: Library,
}

impl Loaded {
    fn load(path: &Path) -> Result<Self, SimulationError> {
        let error = |what: String| SimulationError::SharedLibrary(what);
        // SAFETY: loading libngspice runs no initialization code that could break this process.
        let library = unsafe { Library::new(path.as_os_str()) }
            .map_err(|e| error(format!("cannot load {}: {e}", path.display())))?;
        // SAFETY: the types are those of `sharedspice.h` (ngspice 44).
        let (init, init_sync, command, set_breakpoint) = unsafe {
            let init: NgSpiceInit = *library
                .get("ngSpice_Init")
                .map_err(|e| error(e.to_string()))?;
            let init_sync: NgSpiceInitSync = *library
                .get("ngSpice_Init_Sync")
                .map_err(|e| error(e.to_string()))?;
            let command: NgSpiceCommand = *library
                .get("ngSpice_Command")
                .map_err(|e| error(e.to_string()))?;
            let set_breakpoint: NgSpiceSetBkpt = *library
                .get("ngSpice_SetBkpt")
                .map_err(|e| error(e.to_string()))?;
            (init, init_sync, command, set_breakpoint)
        };
        // SAFETY: the callbacks have the types ngspice expects and live as long as the process.
        let status = unsafe {
            init(
                Some(send_char),
                Some(send_stat),
                Some(controlled_exit),
                Some(send_data),
                Some(send_init_data),
                Some(bg_thread_running),
                ptr::null_mut(),
            )
        };
        // SAFETY: as above; `IDENT` lives as long as the process.
        let status = if status == 0 {
            unsafe {
                init_sync(
                    Some(get_vsrc_data),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    IDENT.as_ptr(),
                    ptr::null_mut(),
                )
            }
        } else {
            status
        };
        let banner = std::mem::take(&mut *lock(&OUTPUT));
        if status != 0 {
            return Err(error(format!("ngSpice_Init returned {status}\n{banner}")));
        }
        Ok(Loaded {
            path: path.to_path_buf(),
            command,
            set_breakpoint,
            _library: library,
        })
    }

    /// Runs a front-end command and returns the status (0 on success).
    fn command(&self, line: &str) -> Result<c_int, SimulationError> {
        let line = CString::new(line).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "a command of ngspice contains a NUL character",
            )
        })?;
        // ngspice may modify the line in place.
        let mut bytes = line.into_bytes_with_nul();
        // SAFETY: a NUL-terminated buffer that outlives the call; the library is loaded.
        Ok(unsafe { (self.command)(bytes.as_mut_ptr().cast()) })
    }
}

/// Collects a line of output, with the `stdout ` / `stderr ` prefix of the library removed.
unsafe extern "C" fn send_char(text: *mut c_char, _id: c_int, _user: *mut c_void) -> c_int {
    if text.is_null() {
        return 0;
    }
    // SAFETY: ngspice passes a NUL-terminated string valid during the call.
    let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    let line = ["stdout", "stderr"]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .map_or(&*text, |rest| rest.strip_prefix(' ').unwrap_or(rest));
    let mut output = lock(&OUTPUT);
    output.push_str(line);
    output.push('\n');
    0
}

/// Progress messages (`tran 45%`) are not needed.
unsafe extern "C" fn send_stat(_text: *mut c_char, _id: c_int, _user: *mut c_void) -> c_int {
    0
}

/// ngspice wants to exit (`quit`, or an error it cannot recover from).
unsafe extern "C" fn controlled_exit(
    _status: c_int,
    _unload: bool,
    _quit: bool,
    _id: c_int,
    _user: *mut c_void,
) -> c_int {
    EXITED.store(true, Ordering::SeqCst);
    0
}

/// A plot starts; its vectors are taken from its first point.
unsafe extern "C" fn send_init_data(
    info: *mut VecInfoAll,
    _id: c_int,
    _user: *mut c_void,
) -> c_int {
    if info.is_null() {
        return 0;
    }
    // SAFETY: ngspice passes a valid structure during the call.
    let plot_type = unsafe { c_string((*info).type_) };
    Active::with(|active, _| active.pending_plot = Some(plot_type));
    0
}

/// An accepted point of the current plot.
unsafe extern "C" fn send_data(
    values: *mut VecValuesAll,
    _count: c_int,
    _id: c_int,
    _user: *mut c_void,
) -> c_int {
    if values.is_null() {
        return 0;
    }
    // SAFETY: ngspice passes `veccount` valid vector values during the call.
    let vectors: &[*mut VecValues] = unsafe {
        let all = &*values;
        if all.vecsa.is_null() {
            return 0;
        }
        std::slice::from_raw_parts(all.vecsa, all.veccount.max(0) as usize)
    };
    Active::with(|active, session| {
        if let Some(plot_type) = active.pending_plot.take() {
            // SAFETY: as above.
            let names: Vec<String> = vectors
                .iter()
                .map(|&v| unsafe { c_string((*v).name) })
                .collect();
            session.begin_plot(&plot_type, &names);
        }
        active.values.clear();
        // SAFETY: as above.
        active
            .values
            .extend(vectors.iter().map(|&v| unsafe { (*v).creal }));
        let set_breakpoint = active.set_breakpoint;
        session.accepted(&active.values, &mut |time| {
            // SAFETY: the library is loaded during the run.
            unsafe { set_breakpoint(time) };
        });
    });
    0
}

/// The value of an `EXTERNAL` voltage source; 0 without a session.
unsafe extern "C" fn get_vsrc_data(
    voltage: *mut f64,
    time: f64,
    name: *mut c_char,
    _id: c_int,
    _user: *mut c_void,
) -> c_int {
    if voltage.is_null() {
        return 0;
    }
    // SAFETY: ngspice passes a NUL-terminated name valid during the call.
    let name = unsafe { c_string(name) };
    let value = Active::with(|_, session| session.source(&name, time)).unwrap_or(0.0);
    // SAFETY: ngspice passes a pointer to the value to set.
    unsafe { *voltage = value };
    0
}

/// A string from ngspice (empty for null).
///
/// # Safety
/// `text` is null or a NUL-terminated string valid during the call.
unsafe fn c_string(text: *const c_char) -> String {
    if text.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Runs are not started in the background thread (`bg_run`), so its state is not needed.
unsafe extern "C" fn bg_thread_running(_running: bool, _id: c_int, _user: *mut c_void) -> c_int {
    0
}
