use super::*;
use defiance_api::ServiceApiV1;

use std::{
    collections::{HashMap, HashSet},
    ffi::{c_char, c_void, CStr, CString},
    path::PathBuf,
    ptr,
    sync::{
        atomic::{AtomicPtr, AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const BASE: usize = 0x100000;

#[derive(Default)]
struct State {
    opened: Vec<u64>,
    closed_sessions: Vec<u64>,
    started: Vec<(u64, usize)>,
    stopped: Vec<u64>,
    active: HashMap<u64, (u64, usize)>,
    logs: Vec<String>,
}

static TEST_LOCK: Mutex<()> = Mutex::new(());
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(100);
static CONFIG_PATH: AtomicPtr<c_char> = AtomicPtr::new(ptr::null_mut());

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn set_config_path(path: &str) {
    let value = CString::new(path).unwrap().into_raw();
    CONFIG_PATH.store(value, Ordering::Release);
}

unsafe extern "C" fn api_log(_: u32, message: *const c_char) {
    let text = if message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .logs
        .push(text);
}

unsafe extern "C" fn module_base(_: *const c_char) -> *mut c_void {
    BASE as *mut c_void
}

unsafe extern "C" fn module_size(_: *mut c_void) -> usize {
    0x1000
}

fn pattern_address(pattern: *const c_char) -> usize {
    if pattern.is_null() {
        return 0;
    }
    let text = unsafe { CStr::from_ptr(pattern) }.to_string_lossy();
    let bytes = text.split_ascii_whitespace().collect::<Vec<_>>();
    match bytes.as_slice() {
        ["90", "C3"] => BASE + 0x10,
        ["90", "90", "C3"] => BASE + 0x20,
        _ => 0,
    }
}

unsafe extern "C" fn find_pattern(_: *mut c_void, _: usize, pattern: *const c_char) -> *mut c_void {
    pattern_address(pattern) as *mut c_void
}

unsafe extern "C" fn find_pattern_at(
    _: *mut c_void,
    _: usize,
    pattern: *const c_char,
    offset: usize,
) -> *mut c_void {
    let address = pattern_address(pattern);
    if address == 0 || offset > 2 {
        ptr::null_mut()
    } else {
        (address + offset) as *mut c_void
    }
}

unsafe extern "C" fn hook(_: *mut c_void, _: *mut c_void, _: *mut *mut c_void) -> i32 {
    1
}

unsafe extern "C" fn hook_exact(
    _: *mut c_void,
    _: *mut c_void,
    _: usize,
    _: *mut *mut c_void,
) -> i32 {
    1
}

unsafe extern "C" fn hook_call(_: *mut c_void, _: *mut c_void, _: *mut *mut c_void) -> i32 {
    1
}

unsafe extern "C" fn unhook(_: *mut c_void) -> i32 {
    1
}

unsafe extern "C" fn rtti_method(_: *const c_char, _: *const c_char) -> *mut c_void {
    ptr::null_mut()
}

unsafe extern "C" fn vtable_slot(_: *const c_char, _: usize) -> *mut c_void {
    ptr::null_mut()
}

unsafe extern "C" fn config_get(_: *const c_char, key: *const c_char) -> *const c_char {
    if key.is_null() || unsafe { CStr::from_ptr(key) }.to_bytes() != b"probe_file" {
        return ptr::null();
    }
    CONFIG_PATH.load(Ordering::Acquire)
}

unsafe extern "C" fn patch_bytes(_: *mut c_void, _: *const u8, _: *const u8, _: usize) -> i32 {
    1
}

static API: Api = Api {
    abi_version: defiance_api::ABI_VERSION,
    reserved: 0,
    log: api_log,
    module_base,
    module_size,
    find_pattern,
    find_pattern_at,
    hook,
    hook_exact,
    hook_call,
    unhook,
    rtti_method,
    vtable_slot,
    config_get,
    patch_bytes,
};

unsafe extern "C" fn capture_open() -> u64 {
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .opened
        .push(session);
    session
}

unsafe extern "C" fn capture_start(
    session: u64,
    request: *const TraceRequestV1,
    handle: *mut u64,
) -> i32 {
    if request.is_null() || handle.is_null() {
        return 1;
    }
    let request = unsafe { &*request };
    let handle_value = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    unsafe { *handle = handle_value };
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    state.started.push((handle_value, request.address));
    state
        .active
        .insert(handle_value, (session, request.address));
    0
}

unsafe extern "C" fn capture_poll(
    handle: u64,
    _: *mut TraceEventV1,
    _: usize,
    stats: *mut TraceStatsV1,
) -> i32 {
    if !stats.is_null() {
        let active = state()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active
            .contains_key(&handle);
        unsafe {
            *stats = TraceStatsV1 {
                active: if active { 1 } else { 0 },
                ..TraceStatsV1::default()
            }
        };
    }
    0
}

unsafe extern "C" fn capture_stop(handle: u64) -> i32 {
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    if state.active.remove(&handle).is_some() {
        state.stopped.push(handle);
        0
    } else {
        1
    }
}

unsafe extern "C" fn capture_close(session: u64) -> i32 {
    state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .closed_sessions
        .push(session);
    0
}

static CAPTURE: TraceCaptureV1 = TraceCaptureV1 {
    open: capture_open,
    start: capture_start,
    poll: capture_poll,
    stop: capture_stop,
    close: capture_close,
};

unsafe extern "C" fn service_register(_: *const c_char, _: u32, _: *const c_void, _: usize) -> i32 {
    0
}

unsafe extern "C" fn service_query(
    _: *const c_char,
    name: *const c_char,
    version: u32,
    min_size: usize,
) -> *const c_void {
    if !name.is_null()
        && unsafe { CStr::from_ptr(name) }.to_bytes() == b"trace-capture"
        && version == 1
        && min_size <= std::mem::size_of::<TraceCaptureV1>()
    {
        &CAPTURE as *const TraceCaptureV1 as *const c_void
    } else {
        ptr::null()
    }
}

static SERVICE_API: ServiceApiV1 = ServiceApiV1 {
    version: 1,
    size: std::mem::size_of::<ServiceApiV1>() as u32,
    register: service_register,
    query: service_query,
};

fn config(name: &str, signature: &str) -> String {
    format!(
        r#"{{"version":1,"probes":[{{"name":"{name}","enabled":true,"signature":"{signature}"}}]}}"#
    )
}

fn wait_until(mut predicate: impl FnMut() -> bool, reason: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if predicate() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {reason}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn create_config_file() -> (PathBuf, PathBuf) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "defiance-diagnostics-worker-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("probes.json");
    (root, path)
}

fn active_addresses() -> HashSet<usize> {
    state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .active
        .values()
        .map(|(_, address)| *address)
        .collect()
}

#[test]
fn worker_reloads_refuses_edits_disarms_on_delete_and_restarts() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    *state().lock().unwrap_or_else(|p| p.into_inner()) = State::default();
    let (root, path) = create_config_file();
    std::fs::write(&path, config("first", "90 C3")).unwrap();
    let path = path.canonicalize().unwrap();
    set_config_path(path.to_str().unwrap());
    assert_eq!(
        unsafe { defiance_feature_sdk::services::accept(&SERVICE_API) },
        0
    );

    assert_eq!(unsafe { start(&API) }, 0);
    wait_until(
        || active_addresses() == HashSet::from([BASE + 0x10]),
        "initial probe to arm",
    );
    assert!(state().lock().unwrap().logs.iter().any(|line| {
        line.contains("probe-file watcher enabled:") && line.contains(&path.display().to_string())
    }));
    let first_handle = state().lock().unwrap().started[0].0;

    std::fs::write(
        &path,
        r#"{"version":1,"probes":[{"name":"broken","enabled":true,"signature":"90"}]"#,
    )
    .unwrap();
    wait_until(
        || {
            state()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .logs
                .iter()
                .any(|line| line.contains("probe file refused"))
        },
        "malformed edit to be refused",
    );
    assert_eq!(active_addresses(), HashSet::from([BASE + 0x10]));
    assert!(!state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .stopped
        .contains(&first_handle));

    std::fs::write(&path, config("second", "90 90 C3")).unwrap();
    wait_until(
        || active_addresses() == HashSet::from([BASE + 0x20]),
        "valid edit to replace the probe",
    );
    let second_handle = state().lock().unwrap().started.last().unwrap().0;
    assert!(state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .stopped
        .contains(&first_handle));

    std::fs::remove_file(&path).unwrap();
    wait_until(
        || active_addresses().is_empty(),
        "deleted config to disarm probes",
    );
    assert!(state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .stopped
        .contains(&second_handle));
    assert!(state().lock().unwrap().logs.iter().any(|line| {
        line.contains("probe file missing:") && line.contains(&path.display().to_string())
    }));

    let first_session = state().lock().unwrap().opened[0];
    stop();
    wait_until(
        || {
            state()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .closed_sessions
                .contains(&first_session)
        },
        "first worker session to close",
    );

    assert_eq!(unsafe { start(&API) }, 0);
    let second_session = state().lock().unwrap().opened[1];
    stop();
    wait_until(
        || {
            state()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .closed_sessions
                .contains(&second_session)
        },
        "second worker session to close",
    );

    let _ = std::fs::remove_file(&path);
    std::fs::remove_dir(&root).unwrap();
}
