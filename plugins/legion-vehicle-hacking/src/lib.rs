//! Allows hacking Legion vehicles while they are repairing themselves.
//!
//! `SmartCursorHacking` accepts a target only when its AI is suspended (an
//! EMP hit), and refuses one already being hacked; `AiUnitHackingState`, the
//! hacker's state, gives up on a target that is not suspended when it
//! arrives. Self-repair never suspends a vehicle, so the plugin changes those
//! checks and leaves the game's target, range and `empvulnerable` checks in
//! place:
//!
//! - both suspension calls are replaced by stubs ([`suspension_stub`] in
//!   game.dll, [`state_stub`] in logic.dll) that make the call and, when it
//!   answers no, answer whether the target's repair-drone facet is repairing
//!   ([`repairing`]). The hacking order then suspends the target for the
//!   hack, as it does a target an EMP suspended.
//! - the cursor's final deny branch is removed.
//!
//! [`sites`] finds all three by signature and refuses a build where any does
//! not resolve.

use core::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN};
use defiance_core::sites::Image;

mod sites;

/// The AI facet's vtable offset of the suspension check.
static SUSPENSION_SLOT: AtomicUsize = AtomicUsize::new(0);
/// The instruction after the suspension call.
static SUSPENSION_RESUME: AtomicUsize = AtomicUsize::new(0);
/// The same for the hacking state's suspension check in logic.dll.
static STATE_SLOT: AtomicUsize = AtomicUsize::new(0);
static STATE_RESUME: AtomicUsize = AtomicUsize::new(0);

/// The entity's facet table (`vt+0xb0`), its repair-drone facet (`+0xa8`)
/// and that facet's repairing flag (`vt+0x68`), as `AiSelfRepairDroneOrder`
/// reads them to end its repair. The same in every build in `bin/`.
const FACETS: usize = 0xb0;
const REPAIR_DRONES: usize = 0xa8;
const DRONES_REPAIRING: usize = 0x68;

type Facets = unsafe extern "C" fn(*const usize) -> *const u8;
type Repairing = unsafe extern "C" fn(*const usize) -> bool;

/// Whether `entity`'s repair drones are repairing it.
unsafe extern "C" fn repairing(entity: *const usize) -> bool {
    if entity.is_null() {
        return false;
    }
    let facets: Facets = core::mem::transmute(*((*entity + FACETS) as *const usize));
    let table = facets(entity);
    if table.is_null() {
        return false;
    }
    let drones = *(table.add(REPAIR_DRONES) as *const *const usize);
    if drones.is_null() {
        return false;
    }
    let flag: Repairing = core::mem::transmute(*((*drones + DRONES_REPAIRING) as *const usize));
    flag(drones)
}

/// Replaces `call [rax+slot]` with RAX the AI facet's vtable, RCX the facet
/// and RDI the target entity, at the cursor check's own aligned stack depth.
/// Leaves the answer in AL for the native `test al, al` it resumes at.
#[unsafe(naked)]
unsafe extern "C" fn suspension_stub() {
    core::arch::naked_asm!(
        "add rax, qword ptr [rip + {slot}]",
        "call qword ptr [rax]",
        "test al, al",
        "jnz 2f",
        "sub rsp, 0x20",
        "mov rcx, rdi",
        "call {repairing}",
        "add rsp, 0x20",
        "2:",
        "jmp qword ptr [rip + {resume}]",
        slot = sym SUSPENSION_SLOT,
        repairing = sym repairing,
        resume = sym SUSPENSION_RESUME,
    );
}

/// [`suspension_stub`] for `AiUnitHackingState`'s target check, whose
/// registers and stack alignment at the call are the same.
#[unsafe(naked)]
unsafe extern "C" fn state_stub() {
    core::arch::naked_asm!(
        "add rax, qword ptr [rip + {slot}]",
        "call qword ptr [rax]",
        "test al, al",
        "jnz 2f",
        "sub rsp, 0x20",
        "mov rcx, rdi",
        "call {repairing}",
        "add rsp, 0x20",
        "2:",
        "jmp qword ptr [rip + {resume}]",
        slot = sym STATE_SLOT,
        repairing = sym repairing,
        resume = sym STATE_RESUME,
    );
}

const BEFORE: [u8; 2] = [0x75, 0x11];
const AFTER: [u8; 2] = [0x90, 0x90];

enum InstallError {
    /// The deny branch did not resolve: this build is not one the plugin
    /// supports.
    UnsupportedBuild(String),
    Failed(String),
}

unsafe fn log(api: &Api, level: u32, text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        (api.log)(level, text.as_ptr());
    }
}

/// Where [`install`] wrote.
struct Installed {
    suspension: usize,
    deny_branch: usize,
    state: usize,
}

/// The base and image of a loaded game module.
unsafe fn module(
    api: &Api,
    name: &core::ffi::CStr,
) -> Result<(*mut u8, Image<'static>), InstallError> {
    let base = (api.module_base)(name.as_ptr()).cast::<u8>();
    if base.is_null() {
        return Err(InstallError::Failed(format!("{name:?} is not loaded")));
    }
    Ok((base, Image::loaded(base, (api.module_size)(base.cast()))))
}

/// Hooks a 6-byte suspension call at `base + rva` with `stub`.
unsafe fn hook(
    api: &Api,
    base: *mut u8,
    rva: usize,
    stub: unsafe extern "C" fn(),
    what: &str,
) -> Result<(), InstallError> {
    let mut trampoline = core::ptr::null_mut();
    if (api.hook_exact)(
        base.add(rva).cast(),
        stub as *mut c_void,
        sites::SUSPENSION_LENGTH,
        &mut trampoline,
    ) != 0
    {
        return Err(InstallError::Failed(format!(
            "{what} at {rva:#x} could not be hooked"
        )));
    }
    Ok(())
}

/// Resolves every site, hooks both suspension calls, then removes the deny
/// branch. A failure leaves the hooks for the loader to remove when `init`
/// refuses.
unsafe fn install(api: &Api) -> Result<Installed, InstallError> {
    let (base, image) = module(api, c"game.dll")?;
    let (logic, logic_image) = module(api, c"logic.dll")?;
    let deny_branch = sites::deny_branch(&image).map_err(InstallError::UnsupportedBuild)?;
    let (suspension, slot) =
        sites::suspension_test(&image).map_err(InstallError::UnsupportedBuild)?;
    let (state, state_slot) =
        sites::state_check(&logic_image).map_err(InstallError::UnsupportedBuild)?;
    SUSPENSION_SLOT.store(slot, Ordering::Release);
    SUSPENSION_RESUME.store(
        base as usize + suspension + sites::SUSPENSION_LENGTH,
        Ordering::Release,
    );
    STATE_SLOT.store(state_slot, Ordering::Release);
    STATE_RESUME.store(
        logic as usize + state + sites::SUSPENSION_LENGTH,
        Ordering::Release,
    );
    hook(
        api,
        base,
        suspension,
        suspension_stub,
        "the suspension test",
    )?;
    hook(api, logic, state, state_stub, "the hacking state's check")?;
    let target = base.add(deny_branch);
    if (api.patch_bytes)(target.cast(), BEFORE.as_ptr(), AFTER.as_ptr(), BEFORE.len()) != 0 {
        return Err(InstallError::Failed(format!(
            "the deny branch at {deny_branch:#x} could not be patched"
        )));
    }
    Ok(Installed {
        suspension,
        deny_branch,
        state,
    })
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = api.as_ref() else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    match install(api) {
        Ok(Installed {
            suspension,
            deny_branch,
            state,
        }) => {
            log(
                api,
                LOG_INFO,
                &format!(
                    "Legion vehicle hacking installed (suspension test at {suspension:#x},                      deny branch at {deny_branch:#x}, hacking state check at logic.dll                      {state:#x})"
                ),
            );
            0
        }
        Err(InstallError::UnsupportedBuild(error)) => {
            log(
                api,
                LOG_WARN,
                &format!("Legion vehicle hacking: not a supported build ({error}); no writes made"),
            );
            1
        }
        Err(InstallError::Failed(error)) => {
            log(
                api,
                LOG_ERROR,
                &format!("Legion vehicle hacking refused: {error}"),
            );
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.legion-vehicle-hacking".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}
