//! Shared refresh events for the native infantry squad information panel.
//!
//! Core owns the native hook and forwards completed refreshes through the
//! loader's observer table. The native entries are resolved from their
//! prologue and validated before the hook is installed.
use crate::plan::{Kind, Plan, Write};
use core::ffi::c_void;
use defiance_api::{Api, SquadPanelFeedV1, SquadPanelRefreshV1, LOG_WARN};
use defiance_core::sites::Image;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

const INFANTRY_PREFIXES: [&[u8]; 2] = [
    b"\x40\x55\x53\x56\x57\x41\x54\x41\x55\x41\x56\x41\x57\x48\x8d\xac\x24\x18\xf7\xff\xff",
    b"\x40\x55\x53\x56\x57\x41\x54\x41\x55\x41\x56\x41\x57\x48\x8d\xac\x24\x38\xf7\xff\xff",
];
const INFANTRY_CLASS: &str = ".?AVUnitManagerSquadInfo@Leonardo@@";
const REFRESH_WRAPPER: &[u8] = b"\x40\x53\x48\x83\xec\x20\x48\x8b\xd9";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sites {
    infantry: usize,
}

fn sites(image: &Image) -> Result<Sites, String> {
    let refresh_of = |class: &str| -> Result<usize, String> {
        let vtable = image.primary_vtable(class)?;
        let wrapper = *vtable
            .methods
            .get(7)
            .ok_or_else(|| format!("{class} has no refresh method"))?;
        image.expect("panel refresh wrapper", wrapper, REFRESH_WRAPPER)?;
        image
            .branch_target(wrapper + REFRESH_WRAPPER.len())
            .ok_or_else(|| format!("{class} refresh wrapper has no call"))
    };
    let infantry = refresh_of(INFANTRY_CLASS)?;
    let infantry_prefix = image
        .image
        .get(infantry..infantry + INFANTRY_PREFIXES[0].len())
        .ok_or("squad refresh prologue runs past game.dll")?;
    if !INFANTRY_PREFIXES
        .iter()
        .any(|prefix| *prefix == infantry_prefix)
    {
        return Err("squad refresh prologue is unsupported".into());
    }
    Ok(Sites { infantry })
}

type Refresh = unsafe extern "system" fn(panel: usize) -> usize;
static INFANTRY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static FEED: OnceLock<&'static SquadPanelFeedV1> = OnceLock::new();

unsafe extern "system" fn infantry_refresh(panel: usize) -> usize {
    let original: Refresh =
        unsafe { core::mem::transmute(INFANTRY_ORIGINAL.load(Ordering::Acquire)) };
    let result = unsafe { original(panel) };
    if let Some(feed) = FEED.get() {
        unsafe { (feed.dispatch)(panel as *mut c_void) };
    }
    result
}

fn loader_tables() -> Option<(&'static SquadPanelRefreshV1, &'static SquadPanelFeedV1)> {
    unsafe {
        Some((
            defiance_feature_sdk::services::query(c"defiance.loader", c"squad-panel-observers", 1)?,
            defiance_feature_sdk::services::squad_panel_feed()?,
        ))
    }
}

fn target(api: &Api) -> Result<usize, String> {
    let base = unsafe { (api.module_base)(c"game.dll".as_ptr()) } as *mut u8;
    if base.is_null() {
        return Err("game.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base.cast()) };
    let image = crate::plan::original_image(base, size).ok_or("game.dll is unreadable")?;
    let resolved = sites(&Image {
        image: &image,
        base: base as usize,
    })?;
    Ok(base as usize + resolved.infantry)
}

fn plan_at(infantry: usize) -> Plan {
    Plan {
        writes: vec![Write {
            target: infantry,
            kind: Kind::Entry(Some(INFANTRY_PREFIXES[0].len())),
            detour: infantry_refresh as Refresh as *mut c_void,
            original: Some(&INFANTRY_ORIGINAL),
        }],
    }
}

/// The shared panel hook, empty unless the loader exposes both service tables
/// and the current game image passes native validation.
pub(crate) fn plan(api: &Api) -> Result<Plan, String> {
    if loader_tables().is_none() {
        return Ok(Plan::default());
    }
    Ok(plan_at(target(api)?))
}

/// Install the hook, then publish its feed and public observer
/// service only after native validation and hook installation succeed.
pub(crate) fn install(api: &Api) {
    let Some((observers, feed)) = loader_tables() else {
        return;
    };
    let plan = match plan(api) {
        Ok(plan) if plan.writes.len() == 1 => plan,
        Ok(_) => return,
        Err(error) => {
            say(api, &format!("squad panel refresh unavailable: {error}"));
            return;
        }
    };
    if crate::plan::apply(api, &plan).is_err() {
        say(api, "squad panel refresh hooks could not be installed");
        return;
    }
    if FEED.set(feed).is_err() || unsafe { (feed.publish)() } != 0 {
        say(api, "squad panel refresh feed was refused");
        return;
    }
    if unsafe { defiance_feature_sdk::services::register(c"squad-panel-refresh", 1, observers) }
        .is_err()
    {
        say(
            api,
            "squad panel refresh observer service could not be registered",
        );
    }
}

fn say(api: &Api, message: &str) {
    let text = std::ffi::CString::new(message).unwrap_or_default();
    unsafe { (api.log)(LOG_WARN, text.as_ptr()) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};
    use std::sync::atomic::AtomicUsize;

    static ORIGINAL_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DISPATCH_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DISPATCH_PANEL: AtomicUsize = AtomicUsize::new(0);
    static ORDER: AtomicUsize = AtomicUsize::new(0);
    static ORIGINAL_ORDER: AtomicUsize = AtomicUsize::new(0);
    static DISPATCH_ORDER: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn original(panel: usize) -> usize {
        ORIGINAL_CALLS.fetch_add(1, Ordering::SeqCst);
        ORIGINAL_ORDER.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
        panel ^ 0x5a5a
    }

    unsafe extern "C" fn dispatch(panel: *mut c_void) {
        DISPATCH_PANEL.store(panel as usize, Ordering::SeqCst);
        DISPATCH_ORDER.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
        DISPATCH_CALLS.fetch_add(usize::from(panel as usize != 0), Ordering::SeqCst);
    }

    unsafe extern "C" fn publish() -> i32 {
        0
    }

    static TEST_FEED: SquadPanelFeedV1 = SquadPanelFeedV1 { publish, dispatch };

    const TABLE: [(&str, usize); 8] = [
        ("gog/2025-12-23", 0x29e6c0),
        ("steam/2025-12-23", 0x2a3a50),
        ("gog/2026-09-14", 0x2a08a0),
        ("steam/2026-09-22", 0x2a5c60),
        ("gog/2026-09-25", 0x2a08a0),
        ("steam/2026-09-25", 0x2a5c60),
        ("gog/2026-10-07", 0x2af2b0),
        ("steam/2026-10-07", 0x2b4640),
    ];

    #[test]
    fn refresh_sites_resolve_and_validate_on_every_supported_build() {
        assert_eq!(TABLE.map(|(build, _)| build), BUILDS);
        for (build, expected) in TABLE {
            let Some(mapped) = reference(build, "game.dll") else {
                continue;
            };
            assert_eq!(
                sites(&Image::mapped(&mapped)),
                Ok(Sites { infantry: expected }),
                "{build}"
            );
        }
    }

    #[test]
    fn changed_or_missing_native_refresh_shape_is_refused() {
        let Some(mapped) = reference(BUILDS[0], "game.dll") else {
            return;
        };
        let original = Image::mapped(&mapped);
        let resolved = sites(&original).unwrap();
        let at = resolved.infantry + 15;
        let mut changed = mapped.image.clone();
        changed[at] ^= 1;
        assert!(
            sites(&Image {
                image: &changed,
                base: mapped.base
            })
            .is_err(),
            "{at:#x}"
        );

        let vtable = original.primary_vtable(INFANTRY_CLASS).unwrap();
        let mut missing = mapped.image.clone();
        missing[vtable.descriptor + 16 + 3] ^= 1;
        assert!(sites(&Image {
            image: &missing,
            base: mapped.base
        })
        .unwrap_err()
        .contains("no primary"));

        let wrapper = vtable.methods[7];
        let mut changed_wrapper = mapped.image.clone();
        changed_wrapper[wrapper + REFRESH_WRAPPER.len()] = 0x90;
        assert!(sites(&Image {
            image: &changed_wrapper,
            base: mapped.base
        })
        .unwrap_err()
        .contains("no call"));
    }

    #[test]
    fn multiple_primary_panel_vtables_are_refused() {
        let Some(mapped) = reference(BUILDS[0], "game.dll") else {
            return;
        };
        let original = Image::mapped(&mapped);
        let vtable = original.primary_vtable(INFANTRY_CLASS).unwrap();
        let mut changed = mapped.image.clone();
        let descriptor = changed.len() + 0x20;
        let locator = changed.len() + 0x100;
        let methods_at = changed.len() + 0x208;
        changed.resize(changed.len() + 0x300, 0);
        changed[descriptor + 16..descriptor + 16 + INFANTRY_CLASS.len()]
            .copy_from_slice(INFANTRY_CLASS.as_bytes());
        changed[locator..locator + 4].copy_from_slice(&0u32.to_le_bytes());
        changed[locator + 4..locator + 8].copy_from_slice(&0u32.to_le_bytes());
        changed[locator + 12..locator + 16].copy_from_slice(&(descriptor as u32).to_le_bytes());
        changed[methods_at - 8..methods_at]
            .copy_from_slice(&((mapped.base + locator) as u64).to_le_bytes());
        for (index, &method) in vtable.methods.iter().enumerate() {
            let at = methods_at + index * 8;
            changed[at..at + 8].copy_from_slice(&((mapped.base + method) as u64).to_le_bytes());
        }
        let error = sites(&Image {
            image: &changed,
            base: mapped.base,
        })
        .unwrap_err();
        assert!(error.contains("2 primary"), "{error}");
    }

    #[test]
    fn infantry_detour_calls_original_once_preserves_result_and_dispatches_afterward() {
        FEED.set(&TEST_FEED).ok().expect("feed is set once");
        ORIGINAL_CALLS.store(0, Ordering::SeqCst);
        DISPATCH_CALLS.store(0, Ordering::SeqCst);
        DISPATCH_PANEL.store(0, Ordering::SeqCst);
        ORDER.store(0, Ordering::SeqCst);
        ORIGINAL_ORDER.store(usize::MAX, Ordering::SeqCst);
        DISPATCH_ORDER.store(usize::MAX, Ordering::SeqCst);
        INFANTRY_ORIGINAL.store(original as Refresh as usize, Ordering::Release);
        let panel = 0x1234;
        let result = unsafe { std::hint::black_box(infantry_refresh(panel)) };
        assert_eq!(result, panel ^ 0x5a5a);
        assert_eq!(ORIGINAL_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(DISPATCH_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(DISPATCH_PANEL.load(Ordering::SeqCst), panel);
        assert!(ORIGINAL_ORDER.load(Ordering::SeqCst) < DISPATCH_ORDER.load(Ordering::SeqCst));
    }
}
