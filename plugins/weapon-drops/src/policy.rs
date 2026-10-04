//! Typed parsing for weapon-drop ammo policies.

use defiance_api::Api;
use std::sync::atomic::{AtomicU8, Ordering};

const PLUGIN_ID: &str = "defiance.weapon-drops";

/// Handling for the outgoing primary's shared-ammo share during replacement.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AmmoPolicy {
    /// Remove the outgoing share from the squad reserve.
    #[default]
    Discard,
    /// Attach the outgoing share to the dropped old-primary pickup.
    Transfer,
    /// Keep the outgoing share as unused squad reserve.
    Retain,
}

/// Handling for shared ammo when the final living squad member dies.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DeathAmmo {
    /// Leave death drops empty and remove the remaining squad reserve.
    #[default]
    Discard,
    /// Distribute compatible unreserved shared rounds once across final slot drops.
    Transfer,
}

/// Parsed startup policy. Both settings default to the existing discard behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Settings {
    pub ammo_policy: AmmoPolicy,
    pub death_ammo: DeathAmmo,
}

static AMMO_POLICY: AtomicU8 = AtomicU8::new(0);
static DEATH_AMMO: AtomicU8 = AtomicU8::new(0);

#[cfg(test)]
thread_local! {
    static TEST_OVERRIDE: std::cell::Cell<Option<Settings>> = const { std::cell::Cell::new(None) };
}

impl AmmoPolicy {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "discard" => Ok(Self::Discard),
            "transfer" => Ok(Self::Transfer),
            "retain" => Ok(Self::Retain),
            _ => Err(format!(
                "invalid ammo_policy `{value}`; expected discard, transfer, or retain"
            )),
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Discard => 0,
            Self::Transfer => 1,
            Self::Retain => 2,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Transfer,
            2 => Self::Retain,
            _ => Self::Discard,
        }
    }
}

impl DeathAmmo {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "discard" => Ok(Self::Discard),
            "transfer" => Ok(Self::Transfer),
            _ => Err(format!(
                "invalid death_ammo `{value}`; expected discard or transfer"
            )),
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Discard => 0,
            Self::Transfer => 1,
        }
    }

    fn from_code(code: u8) -> Self {
        if code == 1 {
            Self::Transfer
        } else {
            Self::Discard
        }
    }
}

/// Parses both canonical settings values independently of the host API.
pub fn parse_settings(ammo_policy: &str, death_ammo: &str) -> Result<Settings, String> {
    Ok(Settings {
        ammo_policy: AmmoPolicy::parse(ammo_policy)?,
        death_ammo: DeathAmmo::parse(death_ammo)?,
    })
}

/// Reads declared startup settings from the host and stores the active values.
///
/// # Safety
/// `api` must be the pointer supplied by the loader during plugin initialization.
pub unsafe fn configure(api: &Api) -> Result<Settings, String> {
    let ammo_policy = unsafe { defiance_feature_sdk::string(api, PLUGIN_ID, "ammo_policy") }
        .map_err(|error| format!("cannot read ammo_policy: {error}"))?;
    let death_ammo = unsafe { defiance_feature_sdk::string(api, PLUGIN_ID, "death_ammo") }
        .map_err(|error| format!("cannot read death_ammo: {error}"))?;
    let settings = parse_settings(&ammo_policy, &death_ammo)?;
    AMMO_POLICY.store(settings.ammo_policy.code(), Ordering::Release);
    DEATH_AMMO.store(settings.death_ammo.code(), Ordering::Release);
    Ok(settings)
}

/// Returns the startup settings most recently installed by [`configure`].
pub fn current() -> Settings {
    #[cfg(test)]
    if let Some(settings) = TEST_OVERRIDE.with(std::cell::Cell::get) {
        return settings;
    }
    Settings {
        ammo_policy: AmmoPolicy::from_code(AMMO_POLICY.load(Ordering::Acquire)),
        death_ammo: DeathAmmo::from_code(DEATH_AMMO.load(Ordering::Acquire)),
    }
}

/// Temporarily supplies settings to tests on the current thread only.
#[cfg(test)]
pub(super) fn set_for_test(settings: Settings) -> TestOverrideGuard {
    let previous = TEST_OVERRIDE.with(|override_value| override_value.replace(Some(settings)));
    TestOverrideGuard(previous)
}

#[cfg(test)]
pub(super) struct TestOverrideGuard(Option<Settings>);

#[cfg(test)]
impl Drop for TestOverrideGuard {
    fn drop(&mut self) {
        TEST_OVERRIDE.with(|override_value| override_value.set(self.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_configuration_defaults_to_legacy_discard_policies() {
        assert_eq!(
            Settings::default(),
            parse_settings("discard", "discard").unwrap()
        );
        assert_eq!(Settings::default(), current());
    }

    #[test]
    fn swap_policies_are_trimmed_and_case_insensitive() {
        assert_eq!(
            parse_settings(" TRANSFER ", "discard").unwrap().ammo_policy,
            AmmoPolicy::Transfer
        );
        assert_eq!(
            parse_settings("retain", "TRANSFER").unwrap().death_ammo,
            DeathAmmo::Transfer
        );
    }

    #[test]
    fn swap_retain_and_death_transfer_can_be_selected_together() {
        assert_eq!(
            parse_settings("retain", "transfer"),
            Ok(Settings {
                ammo_policy: AmmoPolicy::Retain,
                death_ammo: DeathAmmo::Transfer,
            })
        );
    }

    #[test]
    fn invalid_swap_and_death_values_are_rejected() {
        assert!(parse_settings("keep", "discard")
            .unwrap_err()
            .contains("ammo_policy"));
        assert!(parse_settings("discard", "retain")
            .unwrap_err()
            .contains("death_ammo"));
    }

    #[test]
    fn every_swap_policy_is_distinct() {
        assert_ne!(AmmoPolicy::Discard, AmmoPolicy::Transfer);
        assert_ne!(AmmoPolicy::Transfer, AmmoPolicy::Retain);
        assert_ne!(AmmoPolicy::Discard, AmmoPolicy::Retain);
    }

    #[test]
    fn test_override_is_thread_local_and_restored_by_its_guard() {
        let settings = Settings {
            ammo_policy: AmmoPolicy::Retain,
            death_ammo: DeathAmmo::Transfer,
        };
        let guard = set_for_test(settings);
        assert_eq!(current(), settings);
        assert_eq!(
            std::thread::spawn(current).join().unwrap(),
            Settings::default()
        );
        drop(guard);
        assert_eq!(current(), Settings::default());
    }
}
