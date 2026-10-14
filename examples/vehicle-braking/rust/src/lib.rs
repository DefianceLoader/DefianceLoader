//! A complete native plugin using only the public ABI. Read the shared example
//! README for the build, installation and walkthrough.
use core::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use defiance_api::{
    Api, PatchContractEntryV1, PatchContractV1, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO,
    PATCH_KIND_ENTRY,
};

#[cfg(not(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")))]
compile_error!("Build this example for x86_64-pc-windows-msvc.");

const ID: &CStr = c"example.vehicle-braking.rust";
const VERSION: &CStr = c"0.1.0";
const MODULE: &CStr = c"logic.dll";

// The complete arrival callback, with relative operands wildcarded. Checking
// its body guards every native field offset used below, not just the entry.
const ARRIVAL_PATTERN: &CStr = c"48895c2410574883ec20488bf9488bda8b892003000083e901743d83f9020f85????????488b07488bcfff5058f30f1183d00100008b87540300008987640400008b8758030000898768040000488b5c24384883c4205fc3f6824402000001488bcb4889742430488db200020000488bd67407e8????????eb05e8????????488b07488bcfff505848638bc8020000f30f1183d001000085c975050f57d2eb45f68419a702000002744e8d04490f57d24863c8f30f108c8b54020000f30f5c8be8010000f30f10848b5c020000f30f5c83f0010000f30f59c9f30f59c0f30f58c8f30f51d1f30f1083340200000f2fd07706f30f5ed0eb08f30f1015????????f30f1193d4010000f30f5993d0010000f30f1193d00100000f28c2f30f59060f28caf30f1106f30f594e04f30f114e04f30f595608f30f115608488b742430488b5c24384883c4205fc3";
const ENTRY_BEFORE: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x20];

const CHASSIS_MODE: usize = 0x320; // u32: mode 1 runs the final-arrival ramp.
const REQUESTED_SPEED: usize = 0x1d0; // f32: maximum speed multiplied by the ramp.
const ARRIVAL_RATIO: usize = 0x1d4; // f32: zero stopped, one at cruising speed.
const VELOCITY_X: usize = 0x200; // f32: requested velocity's x component.
const VELOCITY_Y: usize = 0x204; // f32: requested velocity's y component.
const VELOCITY_Z: usize = 0x208; // f32: requested velocity's z component.

type Update = unsafe extern "C" fn(*mut u8, *mut u8);

// The loader can fill this output when the managed hook group commits, after
// init returns. Its storage must therefore live as long as the DLL.
static ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static MULTIPLIER: AtomicU32 = AtomicU32::new(1.0f32.to_bits());

struct Setup {
    percent: u32,
    base: *mut c_void,
    site: *mut c_void,
}

unsafe fn host<'a>(api: *const Api) -> Option<&'a Api> {
    let api = api.as_ref()?;
    (api.abi_version == ABI_VERSION && api.reserved == 0).then_some(api)
}

unsafe fn prepare(api: &Api) -> Result<Setup, &'static CStr> {
    let value = (api.config_get)(ID.as_ptr(), c"braking_window_percent".as_ptr());
    if value.is_null() {
        return Err(c"vehicle braking: missing braking_window_percent");
    }
    // config_get supplies canonical decimal text from the manifest's integer
    // setting. Validate the range here; the manifest does not enforce it.
    let bytes = CStr::from_ptr(value).to_bytes();
    let percent = std::str::from_utf8(bytes)
        .ok()
        .filter(|_| !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit))
        .and_then(|text| text.parse::<u32>().ok())
        .filter(|value| (50..=100).contains(value))
        .ok_or(c"vehicle braking: braking_window_percent must be an integer from 50 to 100")?;
    if percent == 100 {
        return Ok(Setup {
            percent,
            base: core::ptr::null_mut(),
            site: core::ptr::null_mut(),
        });
    }
    let base = (api.module_base)(MODULE.as_ptr());
    if base.is_null() {
        return Err(c"vehicle braking: logic.dll is not loaded");
    }
    let size = (api.module_size)(base);
    let site = (api.find_pattern)(base, size, ARRIVAL_PATTERN.as_ptr());
    if site.is_null() {
        return Err(c"vehicle braking: arrival signature missing or ambiguous; no hook installed");
    }
    let offset = (site as usize).checked_sub(base as usize);
    if !offset
        .and_then(|value| value.checked_add(ARRIVAL_PATTERN.to_bytes().len() / 2))
        .is_some_and(|end| end <= size)
    {
        return Err(c"vehicle braking: arrival callback is outside logic.dll");
    }
    if core::slice::from_raw_parts(site.cast::<u8>(), ENTRY_BEFORE.len()) != ENTRY_BEFORE {
        return Err(c"vehicle braking: arrival entry differs; no hook installed");
    }
    Ok(Setup {
        percent,
        base,
        site,
    })
}

unsafe fn read<T: Copy>(object: *mut u8, offset: usize) -> T {
    object.add(offset).cast::<T>().read_unaligned()
}

unsafe fn write(object: *mut u8, offset: usize, value: f32) {
    object.add(offset).cast::<f32>().write_unaligned(value);
}

unsafe extern "C" fn arrival_update(chassis: *mut u8, record: *mut u8) {
    let original: Update = core::mem::transmute(ORIGINAL.load(Ordering::Acquire));
    original(chassis, record);

    // These borrowed objects are valid only inside this callback. Adjust the
    // stock outputs after the original has finished, without retaining pointers.
    let multiplier = f32::from_bits(MULTIPLIER.load(Ordering::Relaxed));
    if read::<u32>(chassis, CHASSIS_MODE) != 1 || multiplier == 1.0 {
        return;
    }
    let ratio = read::<f32>(record, ARRIVAL_RATIO);
    if !(0.0 < ratio && ratio < 1.0) {
        return;
    }
    let next_ratio = (ratio * multiplier).min(1.0);
    let scale = next_ratio / ratio;
    let offsets = [REQUESTED_SPEED, VELOCITY_X, VELOCITY_Y, VELOCITY_Z];
    let values = offsets.map(|offset| read::<f32>(record, offset));
    let next = values.map(|value| value * scale);
    if values[0] < 0.0 || !next.into_iter().all(f32::is_finite) {
        return;
    }
    write(record, ARRIVAL_RATIO, next_ratio);
    for (offset, value) in offsets.into_iter().zip(next) {
        write(record, offset, value);
    }
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = host(api) else {
        return 1;
    };
    let setup = match prepare(api) {
        Ok(setup) => setup,
        Err(message) => {
            (api.log)(LOG_ERROR, message.as_ptr());
            return 1;
        }
    };
    if setup.percent == 100 {
        (api.log)(
            LOG_INFO,
            c"vehicle braking: stock braking; no hook installed".as_ptr(),
        );
        return 0;
    }
    MULTIPLIER.store((100.0 / setup.percent as f32).to_bits(), Ordering::Relaxed);
    if (api.hook_exact)(
        setup.site,
        arrival_update as *mut c_void,
        ENTRY_BEFORE.len(),
        ORIGINAL.as_ptr(),
    ) != 0
    {
        (api.log)(
            LOG_ERROR,
            c"vehicle braking: hook refused; loader owns rollback".as_ptr(),
        );
        return 1;
    }
    let message = format!(
        "vehicle braking: installed ({}% braking window)\0",
        setup.percent
    );
    (api.log)(LOG_INFO, message.as_ptr().cast());
    0
}

/// Declare exactly the entry hook init requests. The loader checks this against
/// staged writes before publishing them. Returned storage lasts for the DLL.
///
/// # Safety
/// api must be null or a valid loader API table.
#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    let Some(api) = host(api) else {
        return core::ptr::null();
    };
    let setup = match prepare(api) {
        Ok(setup) => setup,
        Err(message) => {
            (api.log)(LOG_ERROR, message.as_ptr());
            return core::ptr::null();
        }
    };
    let entries = if setup.percent == 100 {
        core::ptr::null()
    } else {
        Box::into_raw(Box::new(PatchContractEntryV1 {
            module: MODULE.as_ptr(),
            rva: setup.site as usize - setup.base as usize,
            kind: PATCH_KIND_ENTRY,
            before: ENTRY_BEFORE.as_ptr(),
            before_len: ENTRY_BEFORE.len(),
            after: core::ptr::null(),
            after_len: 0,
        }))
    };
    Box::into_raw(Box::new(PatchContractV1 {
        version: 1,
        size: core::mem::size_of::<PatchContractV1>() as u32,
        entries,
        count: usize::from(setup.percent != 100),
    }))
}

/// The fixed C export the loader discovers; the descriptor outlives every call.
#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: ID.as_ptr(),
        version: VERSION.as_ptr(),
        init,
        stop: None,
    })
}
