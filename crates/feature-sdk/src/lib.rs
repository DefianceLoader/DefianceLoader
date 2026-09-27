//! Helpers for the built-in plugins: their patch units ([`units`]), Core's and
//! the loader's services ([`services`]) and crash reporting ([`crash`]).
//!
//! This is also the typed face of `Api::config_get`. The ABI stays a plain
//! C string function, so a plugin written against ABI 5 keeps working; a Rust
//! plugin uses these accessors to get a validated value or an explicit error
//! instead of a nullable string it has to parse itself.
use core::ffi::CStr;
use defiance_api::Api;
pub mod crash;
pub mod services;
pub mod units;
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
