//! More than six experience levels per `scripts/species/exp_levels.txt` row.
use core::ffi::c_void;
use defiance_api::{
    Api, PatchContractV1, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN, PATCH_KIND_CALL,
    PATCH_KIND_ENTRY,
};
use defiance_core::sites::Image;
use defiance_feature_sdk::contract::Patch;
use std::sync::atomic::{AtomicPtr, Ordering};

mod meter;
mod native;
mod sites;

const ID: &str = "defiance.experience-levels";
/// The settings' range for `max_levels`.
const LEVELS: core::ops::RangeInclusive<i64> = 6..=64;

enum InstallError {
    /// The sites did not resolve: this build is not one the plugin supports.
    UnsupportedBuild(String),
    Failed(String),
}

unsafe fn log(api: &Api, level: u32, text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        (api.log)(level, text.as_ptr());
    }
}

/// The settings: `None` when disabled, else the most levels a row may have.
unsafe fn configured(api: &Api) -> Result<Option<usize>, String> {
    let enabled =
        defiance_feature_sdk::boolean(api, ID, "enabled").map_err(|error| error.to_string())?;
    if !enabled {
        return Ok(None);
    }
    let max =
        defiance_feature_sdk::integer(api, ID, "max_levels").map_err(|error| error.to_string())?;
    if !LEVELS.contains(&max) {
        return Err(format!(
            "max_levels = {max} is outside {}..={}",
            LEVELS.start(),
            LEVELS.end()
        ));
    }
    Ok(Some(max as usize))
}

/// logic.dll's base and the sites in it.
unsafe fn resolve(api: &Api) -> Result<(*mut u8, Image<'static>, sites::Sites), InstallError> {
    let base = (api.module_base)(c"logic.dll".as_ptr()).cast::<u8>();
    if base.is_null() {
        return Err(InstallError::Failed("logic.dll is not loaded".into()));
    }
    let image = Image::loaded(base, (api.module_size)(base.cast()));
    let sites = sites::find(&image).map_err(InstallError::UnsupportedBuild)?;
    Ok((base, image, sites))
}

/// The experience meter's sites: game.dll's base, its sites, and the widget
/// move from Core's game-symbol catalog.
unsafe fn resolve_meter(api: &Api) -> Result<(*mut u8, sites::GameSites, usize), String> {
    let move_widget = defiance_feature_sdk::services::game_symbol(
        sites::MOVE_WIDGET_SYMBOL,
        1,
        defiance_api::SYMBOL_USE_CALL,
    )?;
    if move_widget.module != c"game.dll" {
        return Err("Core placed the widget move outside game.dll".into());
    }
    let base = (api.module_base)(c"game.dll".as_ptr()).cast::<u8>();
    if base.is_null() {
        return Err("game.dll is not loaded".into());
    }
    let image = Image::loaded(base, (api.module_size)(base.cast()));
    let sites = sites::find_game(&image)?;
    Ok((base, sites, move_widget.address))
}

/// Every write the plugin makes: the reader's and adapters' entries and the
/// adapters' resolver calls.
fn patches(image: &Image, sites: &sites::Sites) -> Vec<Patch> {
    let entry = |rva, before: &[u8]| Patch {
        module: c"logic.dll",
        rva,
        kind: PATCH_KIND_ENTRY,
        before: before.to_vec(),
        after: None,
    };
    let mut patches = vec![entry(sites.reader, sites::READER_BEFORE)];
    for (adapter, (&rva, &call)) in sites::ADAPTERS
        .iter()
        .zip(sites.adapters.iter().zip(&sites.calls))
    {
        patches.push(entry(rva, adapter.before));
        patches.push(Patch {
            module: c"logic.dll",
            rva: call,
            kind: PATCH_KIND_CALL,
            before: image.image[call..call + 5].to_vec(),
            after: None,
        });
    }
    patches
}

/// Undoes the hooks already placed, last first.
unsafe fn unhook(api: &Api, placed: &[*mut c_void]) {
    for &target in placed.iter().rev() {
        (api.unhook)(target);
    }
}

unsafe fn hook(
    api: &Api,
    placed: &mut Vec<*mut c_void>,
    target: *mut c_void,
    detour: *mut c_void,
    displaced: Option<usize>,
    original: &AtomicPtr<c_void>,
    what: &str,
) -> Result<(), InstallError> {
    let mut out = core::ptr::null_mut();
    let status = match displaced {
        Some(displaced) => (api.hook_exact)(target, detour, displaced, &mut out),
        None => (api.hook_call)(target, detour, &mut out),
    };
    if status != 0 || out.is_null() {
        unhook(api, placed);
        return Err(InstallError::Failed(format!(
            "{what} hook refused; host will finish owned rollback"
        )));
    }
    original.store(out, Ordering::Release);
    placed.push(target);
    Ok(())
}

unsafe fn install(api: &Api) -> Result<String, InstallError> {
    let Some(max) = configured(api).map_err(InstallError::Failed)? else {
        return Ok("experience levels: disabled; no hooks installed".into());
    };
    let (base, _, sites) = resolve(api)?;
    native::API.store((api as *const Api).cast_mut(), Ordering::Release);
    native::MAX_LEVELS.store(max, Ordering::Relaxed);
    native::LOOKUP.store(base.add(sites.lookup).cast(), Ordering::Release);
    let mut placed = Vec::new();
    hook(
        api,
        &mut placed,
        base.add(sites.reader).cast(),
        native::read_row as *mut c_void,
        Some(sites::READER_BEFORE.len()),
        &native::READER,
        "the row reader",
    )?;
    for ((adapter, (&rva, &call)), (detour, original, resolve, resolved)) in sites::ADAPTERS
        .iter()
        .zip(sites.adapters.iter().zip(&sites.calls))
        .zip(native::adapter_hooks())
    {
        hook(
            api,
            &mut placed,
            base.add(call).cast(),
            resolve,
            None,
            resolved,
            adapter.what,
        )?;
        hook(
            api,
            &mut placed,
            base.add(rva).cast(),
            detour,
            Some(adapter.before.len()),
            original,
            adapter.what,
        )?;
    }
    let meter: String = match resolve_meter(api) {
        Ok((game, meter_sites, move_widget)) => {
            let at = |rva: usize| game.add(rva) as usize;
            let _ = meter::GAME.set(meter::Game::at(
                at(meter_sites.factory),
                at(meter_sites.image),
                at(meter_sites.clip),
                at(meter_sites.attach),
                at(meter_sites.bind),
                move_widget,
            ));
            let subscribed = unsafe { defiance_feature_sdk::services::squad_panel_refresh() }
                .is_some_and(|refresh| unsafe {
                    (refresh.subscribe)(Some(meter::panel_refresh), core::ptr::null_mut()) != 0
                });
            if subscribed {
                "meter laid out after Core squad refreshes".into()
            } else {
                log(
                    api,
                    LOG_WARN,
                    "experience levels: Core squad refresh subscription failed; the meter keeps its stock separators",
                );
                "meter keeps its stock separators (refresh subscription unavailable)".into()
            }
        }
        Err(reason) => {
            log(
                api,
                LOG_WARN,
                &format!("experience levels: the meter keeps its stock separators ({reason})"),
            );
            "meter keeps its stock separators".into()
        }
    };
    Ok(format!(
        "experience levels installed (logic+{:#x}): up to {max} levels per row; {meter}",
        sites.reader
    ))
}

#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    let Some(host) = api.as_ref() else {
        return core::ptr::null();
    };
    if host.abi_version != ABI_VERSION || host.reserved != 0 {
        return core::ptr::null();
    }
    let Ok(enabled) = configured(host) else {
        return core::ptr::null();
    };
    let patches = match enabled.map(|_| resolve(host)) {
        None => vec![],
        Some(Ok((_, image, sites))) => patches(&image, &sites),
        // An unsupported build installs nothing, so claims nothing.
        Some(Err(InstallError::UnsupportedBuild(_))) => vec![],
        Some(Err(InstallError::Failed(_))) => return core::ptr::null(),
    };
    defiance_feature_sdk::contract::build(api, patches)
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    match install(api) {
        Ok(message) => {
            log(api, LOG_INFO, &message);
            0
        }
        Err(InstallError::UnsupportedBuild(reason)) => {
            log(
                api,
                LOG_WARN,
                &format!("experience levels: not a supported build ({reason}); no writes made"),
            );
            0
        }
        Err(InstallError::Failed(error)) => {
            log(
                api,
                LOG_ERROR,
                &format!("experience levels refused: {error}"),
            );
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.experience-levels".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

defiance_feature_sdk::crash_handshake!();
defiance_feature_sdk::service_handshake!();
