//! A joined worker owns file reloads and output. Game callbacks never point
//! into this DLL; stopping the worker releases all plugin code references.
use crate::config::{self, Mode, Probe};
use core::ffi::c_void;
use defiance_api::{Api, TraceCaptureV1, TraceEventV1, TraceRequestV1, TraceStatsV1};
use std::{
    collections::BTreeMap,
    ffi::CString,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const ID: &str = "defiance.diagnostics";
const FILE_LIMIT: u64 = 64 * 1024;
const CENSUS_LIMIT: usize = 256;
static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
}

struct Runtime {
    stop: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}

fn log(api: &Api, level: u32, message: &str) {
    if let Ok(text) = CString::new(format!("diagnostics: {message}")) {
        unsafe { (api.log)(level, text.as_ptr()) };
    }
}

fn module_path(module: *mut c_void) -> Result<PathBuf, String> {
    let mut text = vec![0u16; 32768];
    let count =
        unsafe { GetModuleFileNameW(module, text.as_mut_ptr(), text.len() as u32) } as usize;
    if count == 0 || count >= text.len() {
        return Err("cannot resolve module filename".into());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&text[..count])))
}

fn probe_path(text: &str) -> Result<PathBuf, String> {
    if text.is_empty() {
        return Err("probe_file must not be empty".into());
    }
    let path = PathBuf::from(text);
    if path.is_absolute() {
        return Ok(path);
    }
    if path
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(
            "probe_file must be an absolute path or a filename below the config directory".into(),
        );
    }
    Err("loader did not resolve probe_file against its config directory; update the loader".into())
}

/// The loader keeps the API and capture table alive for the process lifetime.
pub(super) unsafe fn start(api: *const Api) -> i32 {
    let Some(api) = (unsafe { api.as_ref() }) else {
        return 1;
    };
    if api.abi_version != defiance_api::ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    let Some(capture) = (unsafe { defiance_feature_sdk::services::trace_capture() }) else {
        log(
            api,
            defiance_api::LOG_ERROR,
            "buffered trace service unavailable; update the loader",
        );
        return 1;
    };
    let path = match unsafe { defiance_feature_sdk::string(api, ID, "probe_file") }
        .map_err(|e| e.to_string())
        .and_then(|text| probe_path(&text))
    {
        Ok(path) => path,
        Err(error) => {
            log(api, defiance_api::LOG_ERROR, &error);
            return 1;
        }
    };
    let session = unsafe { (capture.open)() };
    if session == 0 {
        log(
            api,
            defiance_api::LOG_ERROR,
            "could not open capture session",
        );
        return 1;
    }
    let mut runtime = RUNTIME.lock().unwrap_or_else(|p| p.into_inner());
    if runtime.is_some() {
        unsafe { (capture.close)(session) };
        return 1;
    }
    let stopping = Arc::new(AtomicBool::new(false));
    let worker_stop = stopping.clone();
    let watcher_message = format!("probe-file watcher enabled: {}", path.display());
    // API storage is supplied by the process-lifetime loader.
    let api_address = api as *const Api as usize;
    let worker = match thread::Builder::new()
        .name("defiance-diagnostics".into())
        .spawn(move || {
            let api = unsafe { &*(api_address as *const Api) };
            run(api, capture, session, &path, &worker_stop);
            unsafe { (capture.close)(session) };
        }) {
        Ok(worker) => worker,
        Err(error) => {
            unsafe { (capture.close)(session) };
            log(
                api,
                defiance_api::LOG_ERROR,
                &format!("cannot start worker: {error}"),
            );
            return 1;
        }
    };
    *runtime = Some(Runtime {
        stop: stopping,
        worker,
    });
    log(api, defiance_api::LOG_INFO, &watcher_message);
    0
}

pub(super) fn stop() {
    let runtime = RUNTIME.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(runtime) = runtime {
        runtime.stop.store(true, Ordering::Release);
        runtime.worker.thread().unpark();
        let _ = runtime.worker.join();
    }
}

fn read_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(format!("cannot read probe file: {error}")),
    };
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > FILE_LIMIT {
        return Err("probe file exceeds 64 KiB".into());
    }
    Ok(Some(bytes))
}

fn resolve(api: &Api, probes: Vec<Probe>) -> Result<Vec<Active>, String> {
    let mut resolved = Vec::new();
    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    let mut addresses = Vec::new();
    for probe in probes {
        let module = CString::new(probe.module.as_str()).map_err(|e| e.to_string())?;
        let base = unsafe { (api.module_base)(module.as_ptr()) };
        let size = unsafe { (api.module_size)(base) };
        if base.is_null() || size == 0 {
            return Err(format!("{}: {} is not loaded", probe.name, probe.module));
        }
        if let Some(expected) = &probe.module_sha256 {
            let hash = match hashes.get(&probe.module) {
                Some(hash) => hash.clone(),
                None => {
                    let hash = defiance_core::sha256::file(&module_path(base)?)
                        .map_err(|e| e.to_string())?;
                    hashes.insert(probe.module.clone(), hash.clone());
                    hash
                }
            };
            if hash != *expected {
                return Err(format!(
                    "{}: {} SHA-256 does not match",
                    probe.name, probe.module
                ));
            }
        }
        let address = if let Some(signature) = &probe.signature {
            let signature = CString::new(signature.as_str()).map_err(|e| e.to_string())?;
            let at =
                unsafe { (api.find_pattern_at)(base, size, signature.as_ptr(), probe.site_offset) }
                    as usize;
            if at == 0 {
                return Err(format!(
                    "{}: signature is missing, ambiguous or offset invalid",
                    probe.name
                ));
            }
            if probe
                .rva
                .is_some_and(|rva| at.checked_sub(base as usize) != Some(rva))
            {
                return Err(format!(
                    "{}: signature does not match the declared RVA",
                    probe.name
                ));
            }
            at
        } else {
            (base as usize)
                .checked_add(probe.rva.ok_or("missing RVA")?)
                .ok_or("address overflow")?
        };
        let end = (base as usize)
            .checked_add(size)
            .ok_or("module size overflow")?;
        if address < base as usize || address >= end || addresses.contains(&address) {
            return Err(format!("{}: site outside module or duplicated", probe.name));
        }
        addresses.push(address);
        let mut request = probe.request;
        request.address = address;
        resolved.push(Active::new(probe, request));
    }
    Ok(resolved)
}

struct Active {
    probe: Probe,
    request: TraceRequestV1,
    handle: u64,
    census: BTreeMap<u64, (u64, Option<u64>)>,
    census_overflow: u64,
    stats: TraceStatsV1,
    finished: bool,
    last_report: Instant,
    census_dirty: bool,
}

fn code_address(api: &Api, address: usize) -> String {
    for module in [c"logic.dll", c"game.dll"] {
        let base = unsafe { (api.module_base)(module.as_ptr()) };
        let size = unsafe { (api.module_size)(base) };
        if address
            .checked_sub(base as usize)
            .is_some_and(|rva| !base.is_null() && rva < size)
        {
            return format!(
                "{}+{:#x}",
                module.to_string_lossy(),
                address - base as usize
            );
        }
    }
    format!("{address:#x}")
}

impl Active {
    fn new(probe: Probe, request: TraceRequestV1) -> Self {
        Self {
            probe,
            request,
            handle: 0,
            census: BTreeMap::new(),
            census_overflow: 0,
            stats: TraceStatsV1::default(),
            finished: false,
            last_report: Instant::now(),
            census_dirty: false,
        }
    }
}

fn arm(capture: &TraceCaptureV1, session: u64, probes: &mut [Active]) -> Result<(), String> {
    for probe in probes {
        probe.finished = false;
        let status = unsafe { (capture.start)(session, &probe.request, &mut probe.handle) };
        if status != 0 {
            return Err(format!(
                "{}: capture start refused ({status}); four slots are shared with loader tracing",
                probe.probe.name
            ));
        }
    }
    Ok(())
}

fn disarm(capture: &TraceCaptureV1, probes: &mut [Active]) {
    for probe in probes {
        if probe.handle != 0 {
            unsafe { (capture.stop)(probe.handle) };
            probe.handle = 0;
        }
    }
}

fn replace(
    api: &Api,
    capture: &TraceCaptureV1,
    session: u64,
    active: &mut Vec<Active>,
    mut next: Vec<Active>,
) {
    drain(api, capture, active);
    for probe in active.iter_mut() {
        report_census(api, probe);
    }
    disarm(capture, active);
    match arm(capture, session, &mut next) {
        Ok(()) => {
            *active = next;
            log(
                api,
                defiance_api::LOG_INFO,
                &format!("{} probe(s) armed", active.len()),
            );
            for probe in active.iter() {
                let module = CString::new(probe.probe.module.as_str()).unwrap();
                let base = unsafe { (api.module_base)(module.as_ptr()) };
                let build = module_path(base)
                    .and_then(|path| defiance_core::sha256::file(&path).map_err(|e| e.to_string()))
                    .unwrap_or_else(|_| "unavailable".into());
                log(
                    api,
                    defiance_api::LOG_INFO,
                    &format!(
                        "probe {} site={} module_sha256={build} hits={} every={} interval_ms={}",
                        probe.probe.name,
                        code_address(api, probe.request.address),
                        probe.request.hits,
                        probe.request.every,
                        probe.request.min_interval_ms
                    ),
                );
            }
        }
        Err(error) => {
            disarm(capture, &mut next);
            log(api, defiance_api::LOG_WARN, &error);
            // Partial replacements release their sites before restoring the
            // previous definitions, which receive a fresh hit budget.
            if let Err(error) = arm(capture, session, active) {
                disarm(capture, active);
                active.clear();
                log(
                    api,
                    defiance_api::LOG_ERROR,
                    &format!("previous probes could not be restored: {error}"),
                );
            }
        }
    }
}

fn run(api: &Api, capture: &TraceCaptureV1, session: u64, path: &Path, stopping: &AtomicBool) {
    let mut seen = None;
    let mut read_error = None;
    let mut missing = false;
    let mut active = Vec::new();
    while !stopping.load(Ordering::Acquire) {
        match read_file(path) {
            Ok(bytes) => {
                read_error = None;
                if bytes.is_none() && !missing {
                    log(
                        api,
                        defiance_api::LOG_WARN,
                        &format!("probe file missing: {}; no probes armed", path.display()),
                    );
                }
                missing = bytes.is_none();
                let bytes = bytes.unwrap_or_else(|| br#"{"version":1,"probes":[]}"#.to_vec());
                if seen.as_ref() != Some(&bytes) {
                    let result = std::str::from_utf8(&bytes)
                        .map_err(|e| e.to_string())
                        .and_then(config::parse)
                        .and_then(|probes| resolve(api, probes));
                    match result {
                        Ok(next) => {
                            replace(api, capture, session, &mut active, next);
                            seen = Some(bytes);
                        }
                        Err(error) => {
                            log(
                                api,
                                defiance_api::LOG_WARN,
                                &format!(
                                    "probe file refused; previous probes remain active: {error}"
                                ),
                            );
                            seen = Some(bytes);
                        }
                    }
                }
            }
            Err(error) => {
                if read_error.as_ref() != Some(&error) {
                    log(api, defiance_api::LOG_WARN, &error);
                }
                read_error = Some(error);
            }
        }
        drain(api, capture, &mut active);
        thread::park_timeout(Duration::from_millis(250));
    }
    drain(api, capture, &mut active);
    for probe in &mut active {
        report_census(api, probe);
    }
    disarm(capture, &mut active);
}

fn field_text(kind: u32, bits: u64) -> String {
    match kind {
        defiance_api::TRACE_F32 => f32::from_bits(bits as u32).to_string(),
        defiance_api::TRACE_F64 => f64::from_bits(bits).to_string(),
        defiance_api::TRACE_U8 | defiance_api::TRACE_U16 | defiance_api::TRACE_U32 => {
            bits.to_string()
        }
        _ => format!("{bits:#x}"),
    }
}

fn census(api: &Api, probe: &mut Active, event: &TraceEventV1) {
    let field = |name: &str| {
        probe
            .probe
            .fields
            .iter()
            .position(|field| field.name == name)
            .filter(|index| event.valid_fields & (1 << index) != 0)
            .map(|index| event.values[index])
    };
    let Some(caller) = field("caller") else {
        return;
    };
    let behaviour = field("behaviour");
    if probe.census.len() >= CENSUS_LIMIT && !probe.census.contains_key(&caller) {
        probe.census_overflow += 1;
        return;
    }
    let entry = probe.census.entry(caller).or_insert((0, None));
    entry.0 += 1;
    entry.1 = behaviour;
    probe.census_dirty = true;
    if probe.last_report.elapsed() >= Duration::from_secs(5) {
        report_census(api, probe);
    }
}

fn report_census(api: &Api, probe: &mut Active) {
    if !probe.census_dirty {
        return;
    }
    for (caller, (count, behaviour)) in &probe.census {
        let value = behaviour
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unreadable".into());
        log(
            api,
            defiance_api::LOG_DEBUG,
            &format!(
                "probe {} census caller={} samples={count} behaviour={value}",
                probe.probe.name,
                code_address(api, *caller as usize)
            ),
        );
    }
    probe.census_dirty = false;
    probe.last_report = Instant::now();
}

fn drain(api: &Api, capture: &TraceCaptureV1, probes: &mut [Active]) {
    let mut events = [TraceEventV1::default(); 8];
    for probe in probes {
        if probe.handle == 0 {
            continue;
        }
        // A fixed drain budget keeps busy sites from monopolising the worker.
        for _ in 0..16 {
            let count = unsafe {
                (capture.poll)(
                    probe.handle,
                    events.as_mut_ptr(),
                    events.len(),
                    &mut probe.stats,
                )
            };
            if count < 0 {
                break;
            }
            for event in &events[..count as usize] {
                if probe.probe.mode == Mode::Census {
                    census(api, probe, event);
                    continue;
                }
                let fields = probe
                    .probe
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| {
                        let value = if event.valid_fields & (1 << index) != 0 {
                            if field.name == "caller" {
                                code_address(api, event.values[index] as usize)
                            } else {
                                field_text(field.spec.kind, event.values[index])
                            }
                        } else {
                            "unreadable".into()
                        };
                        format!("{}={value}", field.name)
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let frames = event.frames[..event.frame_count as usize]
                    .iter()
                    .enumerate()
                    .map(|(index, frame)| {
                        format!(
                            "{}{}",
                            if event.scanned_frames & (1 << index) != 0 {
                                "?"
                            } else {
                                ""
                            },
                            code_address(api, *frame)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" < ");
                log(
                    api,
                    defiance_api::LOG_DEBUG,
                    &format!(
                        "probe {} hit={} uptime_ms={} thread={} site={} {fields} stack {frames}",
                        probe.probe.name,
                        event.sequence,
                        event.timestamp_ms,
                        event.thread_id,
                        code_address(api, event.address)
                    ),
                );
            }
            if count < events.len() as i32 {
                break;
            }
        }
        if probe.stats.active == 0 && !probe.finished {
            report_census(api, probe);
            log(api, defiance_api::LOG_INFO, &format!("probe {} finished: hits={} captured={} dropped={} census_overflow={}; disable or edit the probe to release its slot", probe.probe.name, probe.stats.hits, probe.stats.captured, probe.stats.dropped, probe.census_overflow));
            probe.finished = true;
        }
        if probe.last_report.elapsed() >= Duration::from_secs(5) {
            report_census(api, probe);
        }
    }
}

#[cfg(test)]
#[path = "runtime/worker_tests.rs"]
mod worker_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_limit_and_missing_file_are_bounded() {
        let root = std::env::temp_dir().join(format!("defiance-probes-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("probes.json");
        let _ = std::fs::remove_file(&path);
        assert!(read_file(&path).unwrap().is_none());
        std::fs::write(&path, vec![b' '; FILE_LIMIT as usize + 1]).unwrap();
        assert!(read_file(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn paths_and_float_output_keep_their_meaning() {
        assert!(probe_path("../outside.json").is_err());
        assert_eq!(
            field_text(defiance_api::TRACE_F32, 1.5f32.to_bits() as u64),
            "1.5"
        );
        assert_eq!(field_text(defiance_api::TRACE_U64, 0x1234), "0x1234");
    }
}
