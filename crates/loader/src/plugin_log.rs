//! Process-lifetime log callbacks bind an API table to one plugin identity.
//! A plugin can cache the table or callback and use it from any thread.

use defiance_api::Api;
use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::sync::{Mutex, OnceLock};

type Log = unsafe extern "C" fn(u32, *const c_char);
const MAX_SOURCES: usize = 256;
static SOURCES: [OnceLock<String>; MAX_SOURCES] = [const { OnceLock::new() }; MAX_SOURCES];
type ApiCache = HashMap<(String, usize), &'static Api>;
static APIS: OnceLock<Mutex<ApiCache>> = OnceLock::new();

unsafe extern "C" fn scoped_log<const SLOT: usize>(level: u32, message: *const c_char) {
    let Some(source) = SOURCES[SLOT].get() else {
        return;
    };
    let text = if message.is_null() {
        std::borrow::Cow::Borrowed("")
    } else {
        unsafe { CStr::from_ptr(message) }.to_string_lossy()
    };
    crate::log::plugin(level, source, &text);
}

macro_rules! callbacks {
    ($($slot:expr),* $(,)?) => {
        [$(scoped_log::<$slot> as Log),*]
    };
}

// Each callback has its own immutable source slot, including across DLL reloads.
static CALLBACKS: [Log; MAX_SOURCES] = callbacks![
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
    50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73,
    74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97,
    98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
    117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135,
    136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154,
    155, 156, 157, 158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173,
    174, 175, 176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192,
    193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211,
    212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230,
    231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249,
    250, 251, 252, 253, 254, 255,
];

/// Preserve all host API fields while binding log calls to the plugin ID.
/// Tables and callbacks remain valid for the process lifetime. IDs reuse their
/// source slot; a different base API gets a separate table with those functions.
pub fn bind(api: &'static Api, id: &str) -> Result<&'static Api, String> {
    let id = id.to_ascii_lowercase();
    let key = (id.clone(), api as *const Api as usize);
    let mut apis = APIS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(bound) = apis.get(&key) {
        return Ok(*bound);
    }
    let slot = SOURCES
        .iter()
        .position(|source| source.get() == Some(&id))
        .or_else(|| SOURCES.iter().position(|source| source.get().is_none()))
        .ok_or_else(|| format!("plugin logger limit ({MAX_SOURCES} distinct IDs) reached"))?;
    let _ = SOURCES[slot].set(id);
    let mut bound = *api;
    bound.log = CALLBACKS[slot];
    let bound = Box::leak(Box::new(bound));
    apis.insert(key, bound);
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logger_capacity_refuses_new_ids_without_reassigning_callbacks() {
        const CHILD: &str = "DEFIANCE_LOG_CAPACITY_TEST";
        if std::env::var_os(CHILD).is_some() {
            let api = Box::leak(Box::new(crate::resolve::build_api()));
            let first = bind(api, "test.first").unwrap();
            for index in 1..MAX_SOURCES {
                bind(api, &format!("test.capacity-{index}")).unwrap();
            }
            assert!(matches!(bind(api, "test.excess"), Err(error) if error.contains("limit")));
            let rebound = bind(api, "test.first").unwrap();
            assert!(std::ptr::eq(first, rebound));
            unsafe { (first.log)(defiance_api::LOG_INFO, c"capacity-kept-source".as_ptr()) };
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "plugin_log::tests::logger_capacity_refuses_new_ids_without_reassigning_callbacks",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "child failed: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("[test.first] capacity-kept-source")
        );
    }

    #[test]
    fn cached_callbacks_keep_plugin_identity_on_workers_and_after_rebinding() {
        const CHILD: &str = "DEFIANCE_LOG_FILTER_TEST";
        if let Ok(mode) = std::env::var(CHILD) {
            let (include, exclude, level) = match mode.as_str() {
                "include" => ("TEST.LOG-A", "", "info"),
                "exclude" => ("", "test.log-a", "info"),
                "both" => ("test.log-a,test.log-b", "test.log-a", "info"),
                "severity" => ("", "", "error"),
                _ => ("", "", "info"),
            };
            crate::log::set_plugin_filter(include, exclude);
            crate::log::set_level(level);
            let dir =
                std::path::PathBuf::from(std::env::var_os("DEFIANCE_LOG_FILTER_DIR").unwrap());
            let (paths, _) = crate::config::paths::Paths::resolve(
                &dir,
                &crate::config::parse::parse("root = ."),
            );
            crate::log::relocate(&paths);
            let api = Box::leak(Box::new(crate::resolve::build_api()));
            let a = bind(api, "test.log-a").unwrap();
            let b = bind(api, "test.log-b").unwrap();
            assert!(std::ptr::eq(a, bind(api, "TEST.LOG-A").unwrap()));
            let replacement = Box::leak(Box::new(crate::resolve::build_api()));
            let rebound = bind(replacement, "test.log-a").unwrap();
            assert!(!std::ptr::eq(a, rebound));
            assert_eq!(a.log as usize, rebound.log as usize);
            std::thread::spawn(move || unsafe {
                (a.log)(defiance_api::LOG_INFO, c"worker-a".as_ptr());
                (b.log)(defiance_api::LOG_INFO, c"worker-b".as_ptr());
                (rebound.log)(defiance_api::LOG_INFO, c"reloaded-a".as_ptr());
                (a.log)(
                    defiance_api::LOG_INFO,
                    c"[test.log-b] claimed-source".as_ptr(),
                );
            })
            .join()
            .unwrap();
            crate::log::info("host-message");
            return;
        }
        for (mode, a, b, host) in [
            ("all", true, true, true),
            ("include", true, false, true),
            ("exclude", false, true, true),
            ("both", false, true, true),
            ("severity", false, false, false),
        ] {
            let dir = std::env::temp_dir().join(format!(
                "defiance-plugin-log-test-{}-{}-{mode}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "plugin_log::tests::cached_callbacks_keep_plugin_identity_on_workers_and_after_rebinding", "--nocapture"])
                .env(CHILD, mode).env("DEFIANCE_LOG_FILTER_DIR", &dir).output().unwrap();
            assert!(output.status.success(), "child failed: {output:?}");
            let text = std::fs::read_to_string(dir.join("logs/defiance-loader.log")).unwrap();
            assert_eq!(text.contains("[test.log-a] worker-a"), a, "{mode}: {text}");
            assert_eq!(text.contains("[test.log-b] worker-b"), b, "{mode}: {text}");
            assert_eq!(
                text.contains("[test.log-a] reloaded-a"),
                a,
                "{mode}: {text}"
            );
            assert_eq!(
                text.contains("[test.log-a] [test.log-b] claimed-source"),
                a,
                "{mode}: {text}"
            );
            assert_eq!(text.contains("host-message"), host, "{mode}: {text}");
            assert!(dir.starts_with(std::env::temp_dir()));
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
