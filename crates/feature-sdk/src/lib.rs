//! Built-in feature entry points. The core plugin prepares shared storage;
//! each feature installs its own spans through the host's ownership API.
//!
//! This is also the typed face of `Api::config_get`. The ABI stays a plain
//! C string function, so a plugin written against ABI 5 keeps working; a Rust
//! plugin uses these accessors to get a validated value or an explicit error
//! instead of a nullable string it has to parse itself.
use core::ffi::{c_void, CStr};
use defiance_api::{Api, ABI_VERSION, LOG_ERROR};
pub mod crash;
pub mod services;
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}
pub unsafe fn install(api: *const Api, feature: u32) -> i32 {
    if api.is_null() {
        return 1;
    }
    let host = unsafe { &*api };
    if host.abi_version != ABI_VERSION || host.reserved != 0 {
        return 1;
    }
    let name: Vec<u16> = "defiance_plugin_core.dll\0".encode_utf16().collect();
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    let entry = if module.is_null() {
        core::ptr::null_mut()
    } else {
        unsafe { GetProcAddress(module, b"defiance_install_feature_v1\0".as_ptr()) }
    };
    if entry.is_null() {
        unsafe {
            (host.log)(
                LOG_ERROR,
                b"built-in feature needs defiance_plugin_core.dll\0"
                    .as_ptr()
                    .cast(),
            )
        };
        return 1;
    }
    let install: unsafe extern "C" fn(*const Api, u32) -> i32 =
        unsafe { core::mem::transmute(entry) };
    unsafe { install(api, feature) }
}

/// Install the Rust pickup callback through Core's verified site.
///
/// # Safety
/// `api` comes from plugin init; `detour` must be a permanent callback with the
/// game's pickup chooser ABI. Call only during pickup plugin initialization.
pub unsafe fn install_pickup_rust(api: *const Api, detour: *mut c_void) -> i32 {
    if api.is_null() || detour.is_null() {
        return 1;
    }
    let host = unsafe { &*api };
    if host.abi_version != ABI_VERSION || host.reserved != 0 {
        return 1;
    }
    let name: Vec<u16> = "defiance_plugin_core.dll\0".encode_utf16().collect();
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    let entry = if module.is_null() {
        core::ptr::null_mut()
    } else {
        unsafe { GetProcAddress(module, b"defiance_install_pickup_rust_v1\0".as_ptr()) }
    };
    if entry.is_null() {
        unsafe {
            (host.log)(
                LOG_ERROR,
                b"Rust pickup needs a Core with the verified pickup hook service\0"
                    .as_ptr()
                    .cast(),
            )
        };
        return 1;
    }
    let install: unsafe extern "C" fn(*const Api, *mut c_void) -> i32 =
        unsafe { core::mem::transmute(entry) };
    unsafe { install(api, detour) }
}

/// Internal migration helper for built-in ordinary function replacements.
/// # Safety
/// Call during init with permanent detours matching each named game's function ABI.
pub unsafe fn install_native(
    api: *const Api,
    feature: u32,
    replacements: &[defiance_api::NativeReplacementV1],
) -> i32 {
    if api.is_null() || (*api).abi_version != ABI_VERSION || (*api).reserved != 0 {
        return 1;
    }
    let name: Vec<u16> = "defiance_plugin_core.dll\0".encode_utf16().collect();
    let module = GetModuleHandleW(name.as_ptr());
    if module.is_null() {
        return 1;
    }
    let entry = GetProcAddress(module, b"defiance_install_native_v1\0".as_ptr());
    if entry.is_null() {
        return 1;
    }
    let install: unsafe extern "C" fn(
        *const Api,
        u32,
        *const defiance_api::NativeReplacementV1,
        usize,
    ) -> i32 = core::mem::transmute(entry);
    install(api, feature, replacements.as_ptr(), replacements.len())
}

/// Why a typed configuration lookup failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// There is no value for `(plugin_id, key)`: the setting was not declared,
    /// the plugin is unknown, or the owning plugin is blocked by invalid
    /// configuration. A blocked plugin must not run, so this is not a default.
    Unavailable,
    /// The host returned a value that is not valid for the requested type. This
    /// is a declaration/reader mismatch, not something a player can fix.
    Invalid(String),
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConfigError::Unavailable => write!(formatter, "no configuration value"),
            ConfigError::Invalid(message) => write!(formatter, "{message}"),
        }
    }
}

/// The raw value the host holds for `(plugin_id, key)`, or `None`. The pointer
/// is owned by the host and stays valid for the process's life; the returned
/// string is a copy.
///
/// # Safety
/// `api` must be the pointer the host passed to `init`.
pub unsafe fn raw(api: &Api, plugin_id: &str, key: &str) -> Option<String> {
    let plugin = std::ffi::CString::new(plugin_id).ok()?;
    let key = std::ffi::CString::new(key).ok()?;
    let value = unsafe { (api.config_get)(plugin.as_ptr(), key.as_ptr()) };
    if value.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// A string setting. An absent setting is [`ConfigError::Unavailable`]; a
/// declared `enabled` default is never returned, because defaults are already
/// materialized by the host.
///
/// # Safety
/// `api` must be the pointer the host passed to `init`.
pub unsafe fn string(api: &Api, plugin_id: &str, key: &str) -> Result<String, ConfigError> {
    unsafe { raw(api, plugin_id, key) }.ok_or(ConfigError::Unavailable)
}

/// A boolean setting, in the same forms the loader accepts
/// (`true`/`yes`/`on`/`1`, `false`/`no`/`off`/`0`).
///
/// # Safety
/// `api` must be the pointer the host passed to `init`.
pub unsafe fn boolean(api: &Api, plugin_id: &str, key: &str) -> Result<bool, ConfigError> {
    let text = unsafe { raw(api, plugin_id, key) }.ok_or(ConfigError::Unavailable)?;
    parse_bool(&text)
}

/// An integer setting.
///
/// # Safety
/// `api` must be the pointer the host passed to `init`.
pub unsafe fn integer(api: &Api, plugin_id: &str, key: &str) -> Result<i64, ConfigError> {
    let text = unsafe { raw(api, plugin_id, key) }.ok_or(ConfigError::Unavailable)?;
    parse_integer(&text)
}

/// The boolean forms, shared with the host's own parser.
pub fn parse_bool(text: &str) -> Result<bool, ConfigError> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        other => Err(ConfigError::Invalid(format!("`{other}` is not a boolean"))),
    }
}

/// Integer parsing, deliberately stricter than `f64` so `1.0` is refused.
pub fn parse_integer(text: &str) -> Result<i64, ConfigError> {
    text.trim()
        .parse()
        .map_err(|_| ConfigError::Invalid(format!("`{text}` is not an integer")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booleans_accept_the_legacy_forms() {
        assert_eq!(parse_bool(" ON "), Ok(true));
        assert_eq!(parse_bool("No"), Ok(false));
        assert!(parse_bool("maybe").is_err());
    }

    #[test]
    fn integers_are_strict() {
        assert_eq!(parse_integer(" 42 "), Ok(42));
        assert!(parse_integer("1.0").is_err());
        assert!(parse_integer("0x10").is_err());
    }
}
