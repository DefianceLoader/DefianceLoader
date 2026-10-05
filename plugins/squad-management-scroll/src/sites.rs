//! Where the hooked, called and patched native code lives, found by signature
//! and RTTI instead of looked up by build hash.
//!
//! The hooked entries and most called functions are found by byte signature
//! (`tools/site_signatures.py`'s encoding). The five vtables are the primary
//! vtables of their RTTI classes; the destructors and both refreshes come from
//! their slots, and the remaining helpers, the upgrade key static and the
//! class offsets are read from the instructions that use them. Each must
//! resolve to exactly one place and every hooked entry must start with the
//! bytes its hook relocates, so a build where any site moved or changed shape
//! resolves to an error and the plugin writes nothing.
use defiance_core::sites::{sig, Image, Signature};

/// The game.dll sites, as rvas, and the class offsets the plugin reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sites {
    /// TrainingWindow show, hooked after the stock body.
    pub training_show: usize,
    /// TrainingWindow's two-row item placer.
    pub training_layout: usize,
    /// The panels' shared event dispatch (vtable slot 1).
    pub dispatch: usize,
    /// The squad panel's destructor (vtable slot 0).
    pub destroy: usize,
    /// The ammunition column bind.
    pub ammo: usize,
    /// The weapon column bind.
    pub weapon: usize,
    /// The script service lookup the weapon bind calls.
    pub script: usize,
    /// The widget listener registration.
    pub listen: usize,
    /// The slider thumb layout.
    pub thumb: usize,
    /// GuiSliderCtrl's event dispatch (vtable slot 1).
    pub slider_dispatch: usize,
    /// The perk column bind.
    pub perk: usize,
    /// The upgrade_slot_* column bind, shared by both panels: (widget, display).
    pub upgrade: usize,
    /// TrainingWindow's destructor (vtable slot 0).
    pub training_destroy: usize,
    /// The vehicle panel's destructor (vtable slot 0).
    pub vehicle_destroy: usize,
    /// The vehicle panel's refresh, called by its vtable slot 7.
    pub vehicle_refresh: usize,
    /// The squad panel's refresh, called by its vtable slot 7.
    pub refresh: usize,
    /// The getter of the training table's key: it stores a static pointer in
    /// its out-parameter, and pointer + 8 resolves training names.
    pub training_key: usize,
    /// The squad and vehicle panels' drop-target choosers, which the army
    /// presets window calls as an item drag starts: (panel, &out, item).
    pub squad_chooser: usize,
    pub vehicle_chooser: usize,
    pub training_vtable: usize,
    /// The UnitManagerSquadInfo vtable.
    pub panel_vtable: usize,
    pub vehicle_vtable: usize,
    pub slider_vtable: usize,
    pub slider_ctrl_vtable: usize,
    /// The lazily-initialised upgrade key static: the stock upgrade loop reads
    /// the pointer here and passes pointer + 8 to the script service's display
    /// resolver. The stock refresh initialises it before the rerun.
    pub upgrade_key: usize,
    /// The context's service member, as the weapon bind reads it.
    pub context_service: usize,
    /// The vtable slot of the squad's rank (from its experience) on the stock
    /// perk refresh's service object.
    pub perk_limit: usize,
    /// Where an entry of an upgrade record's squad list names its squad (a
    /// std::string; empty fits every squad), as the squad chooser's match
    /// reads it.
    pub squad_fit_name: usize,
}

const TRAINING_SHOW: Signature = sig(
    "TrainingWindow show",
    &[("48895c24084889742410574883ec20488991f0010000488bf1", 0x0)],
);
const TRAINING_LAYOUT: Signature = sig(
    "TrainingWindow item placer",
    &[
        ("48895c241048896c241856574154415641574883ec504d8be0", 0x0),
        (
            "48895c241048896c2418488974242057415641574883ec50498bd8",
            0x0,
        ),
    ],
);
const DISPATCH: Signature = sig(
    "panel event dispatch",
    &[(
        "418b014c8bd13d0a0200000f87????????0f84????????3d00020000",
        0x0,
    )],
);
const DESTROY: Signature = sig(
    "squad panel destructor",
    &[(
        "48895c2408574883ec208bda488bf9e8????????f6c301740dba58070000",
        0x0,
    )],
);
const VEHICLE_DESTROY: Signature = sig(
    "vehicle panel destructor",
    &[(
        "48895c2408574883ec208bda488bf9e8????????f6c301740dba90070000",
        0x0,
    )],
);
const TRAINING_DESTROY: Signature = sig(
    "TrainingWindow destructor",
    &[(
        "48895c24084889742410574883ec20488d05????????488bd94889018bf24881c1d8010000",
        0x0,
    )],
);
const AMMO: Signature = sig(
    "ammunition column bind",
    &[(
        "48895c2408574883ec40488bd9488991d0010000488db980000000",
        0x0,
    )],
);
const WEAPON: Signature = sig(
    "weapon column bind",
    &[(
        "48895c2410488974241855574156488dac2470feffff4881ec90020000488bf9",
        0x0,
    )],
);
const LISTEN: Signature = sig(
    "listener registration",
    &[("4883ec2880795c00488bc2744848895c2420488b99b0000000", 0x0)],
);
const THUMB: Signature = sig(
    "slider thumb layout",
    &[(
        "48895c241048896c2418488974242057415641574883ec504c8bf9",
        0x0,
    )],
);
const SLIDER_DISPATCH: Signature = sig(
    "slider event dispatch",
    &[("48895c2418555657415641574883ec40488bf94533ff418b09", 0x0)],
);
const PERK: Signature = sig(
    "perk column bind",
    &[("48895c240848896c24184889742420574883ec40488bf2488be9", 0x0)],
);
const UPGRADE: Signature = sig(
    "upgrade column bind",
    &[("48895c2410574883ec60488bda488bf9c744247000000000", 0x0)],
);
const SQUAD_CHOOSER: Signature = sig(
    "squad drop-target chooser",
    &[
        (
            "48895c241848895424105556574154415541564157488bec4881ec80000000498bf0",
            0x0,
        ),
        (
            "48895c241848895424105556574154415541564157488bec4881ec800000004d8bf0",
            0x0,
        ),
    ],
);
const VEHICLE_CHOOSER: Signature = sig(
    "vehicle drop-target chooser",
    &[(
        "48895c2410488974241848897c2420554154415541564157488bec4881ec80000000498bf0",
        0x0,
    )],
);
/// The perk refresh's `mov r9, [rcx/rax + perk_limit]` before its rank call;
/// the site is the displacement.
const PERK_LIMIT: Signature = sig(
    "perk limit slot",
    &[
        ("488b10488bc8ff5238488b104c8b8a????????4d8b86b8", 15),
        ("488b10488bc8ff5238488b084c8b89????????4d8b86b8", 15),
    ],
);
/// The squad chooser's match over an upgrade record's squad fits: `cmp [rsi +
/// size], 0` then `lea rcx, [rsi + size - 0x10]`, the name. The site is the
/// `lea` displacement.
const SQUAD_FIT_NAME: Signature = sig(
    "squad fit name",
    &[(
        "4c8ba170020000488bb968020000488b5c2430493bfc7474488b374883be????????007431488d8e????????",
        40,
    )],
);

// The class layouts native.rs reads as constants, checked where stock code
// uses them so a build that moved a field resolves to an error.

/// `cmp byte [r14 + 0x210], 0` (`PANEL_LOCKS`) and `cmp [r14 + 0x352], r12b/r13b`
/// (`PANEL_MAY_PICK`) in the perk refresh, after its perk limit read.
const PANEL_LOCKS: Signature = sig("panel locks check", &[("4180be1002000000", 0)]);
const PANEL_MAY_PICK: Signature = sig("panel may-pick check", &[("4538??52030000", 0)]);
/// How far past the perk limit read both checks lie.
const PERK_REFRESH_SPAN: usize = 0x800;
/// The vehicle chooser's match loads the first of an upgrade record's nine
/// vehicle fit lists (`FITS`, begin at 0x190, end at 0x198) from `rcx`, then
/// the other eight from `rax`, 0x18 apart, and names each entry at +0x28.
const VEHICLE_FITS: Signature = sig(
    "vehicle fit walk",
    &[("4c8ba1980100004c8bb190010000488b5d", 0)],
);
const VEHICLE_FITS_SPAN: usize = 0x500;
/// `lea rcx, [r15 + 0x28]`, a fit entry's name.
const FIT_ENTRY_NAME: &[u8] = b"\x49\x8d\x4f\x28";
/// The TrainingWindow constructor's walk of the training table, which
/// `trainings_available` repeats: the script service from holder vt+0x58, the
/// table path from its vt+0x20 with the training key + 8, the tables from
/// holder vt+0x70, the table from their vt+0x20, columns at +0x18 and rows at
/// +0x20. The site is the training key call.
const TRAINING_ROWS: Signature = sig(
    "training table walk",
    &[(
        "ff5058488bf8488b08488b5920488d4d60e8????????488b104883c208488bcfffd3488bd0488d4dd0e8????????90498b4500498bcdff5070488b084c8b4120488d55d0488bc841ffd0488b7820488b4818",
        0x11,
    )],
);

const TRAINING: &str = ".?AVTrainingWindow@Leonardo@@";
const PANEL: &str = ".?AVUnitManagerSquadInfo@Leonardo@@";
const VEHICLE: &str = ".?AVUnitManagerVehicleInfo@Leonardo@@";
const SLIDER: &str = ".?AVGuiSliderWidget@Leonardo@@";
const SLIDER_CTRL: &str = ".?AVGuiSliderCtrl@Leonardo@@";

/// The entry bytes each hook relocates. The squad refresh has two stack
/// frames across the builds.
pub(crate) const TRAINING_SHOW_PREFIX: &[u8] =
    b"\x48\x89\x5c\x24\x08\x48\x89\x74\x24\x10\x57\x48\x83\xec\x20\x48\x89\x91\xf0\x01\x00\x00";
pub(crate) const REFRESH_PREFIXES: [&[u8]; 2] = [
    b"\x40\x55\x53\x56\x57\x41\x54\x41\x55\x41\x56\x41\x57\x48\x8d\xac\x24\x18\xf7\xff\xff",
    b"\x40\x55\x53\x56\x57\x41\x54\x41\x55\x41\x56\x41\x57\x48\x8d\xac\x24\x38\xf7\xff\xff",
];
pub(crate) const VEHICLE_REFRESH_PREFIX: &[u8] =
    b"\x40\x55\x53\x56\x57\x41\x54\x41\x55\x41\x56\x41\x57\x48\x8d\xac\x24\xd8\xf9\xff\xff";
pub(crate) const THUMB_PREFIX: &[u8] =
    b"\x48\x89\x5c\x24\x10\x48\x89\x6c\x24\x18\x48\x89\x74\x24\x20\x57\x41\x56\x41\x57\x48\x83\xec\x50";
pub(crate) const SQUAD_CHOOSER_PREFIX: &[u8] =
    b"\x48\x89\x5c\x24\x18\x48\x89\x54\x24\x10\x55\x56\x57\x41\x54\x41\x55\x41\x56\x41\x57";
pub(crate) const VEHICLE_CHOOSER_PREFIX: &[u8] =
    b"\x48\x89\x5c\x24\x10\x48\x89\x74\x24\x18\x48\x89\x7c\x24\x20\x55\x41\x54\x41\x55\x41\x56";

/// Both panels' vtable slot 7 is a wrapper, `push rbx; sub rsp, 0x20; mov rbx,
/// rcx`, whose first call is the refresh.
const REFRESH_WRAPPER: &[u8] = b"\x40\x53\x48\x83\xec\x20\x48\x8b\xd9";
/// `mov rcx, rax; call script` in the weapon bind.
const SCRIPT_CALL: (usize, &[u8]) = (0x216, b"\x48\x8b\xc8\xe8");
/// The script lookup's and the training key getter's entries, which the
/// calls above lead to.
const SCRIPT_PREFIX: &[u8] =
    b"\x40\x55\x56\x57\x48\x83\xec\x40\x48\x8b\x01\x48\x8b\xf9\x44\x8b\x05";
const TRAINING_KEY_CALL: usize = 0x48;
const TRAINING_KEY_PREFIX: &[u8] =
    b"\x40\x57\x48\x83\xec\x40\x65\x48\x8b\x04\x25\x58\x00\x00\x00\x48\x8b\xf9\x8b\x15";
/// `mov rax, [rdi+0x1b0]; mov rcx, [rax + context_service]` in the weapon bind.
const CONTEXT_SERVICE: (usize, &[u8]) = (0x1fb, b"\x48\x8b\x87\xb0\x01\x00\x00\x48\x8b\x88");
/// `mov rdx, [rip + upgrade_key]; add rdx, 8` in the vehicle refresh.
const UPGRADE_KEY: usize = 0xca9;
const UPGRADE_KEY_LOAD: &[u8] = b"\x48\x8b\x15";
const UPGRADE_KEY_ADD: &[u8] = b"\x48\x83\xc2\x08";

/// The sites, or why this build is not supported.
pub(crate) fn sites(image: &Image) -> Result<Sites, String> {
    let training = image.primary_vtable(TRAINING)?;
    let panel = image.primary_vtable(PANEL)?;
    let vehicle = image.primary_vtable(VEHICLE)?;
    let slider = image.primary_vtable(SLIDER)?;
    let slider_ctrl = image.primary_vtable(SLIDER_CTRL)?;
    let slot = |vtable: &defiance_core::rtti::Vtable, class: &str, slot: usize| {
        vtable
            .methods
            .get(slot)
            .copied()
            .ok_or_else(|| format!("{class} has no method {slot}"))
    };
    let refresh_of = |vtable: &defiance_core::rtti::Vtable, class: &str| {
        let wrapper = slot(vtable, class, 7)?;
        image.expect(
            &format!("{class} refresh wrapper"),
            wrapper,
            REFRESH_WRAPPER,
        )?;
        image
            .branch_target(wrapper + REFRESH_WRAPPER.len())
            .ok_or_else(|| format!("{class} refresh wrapper has no call"))
    };
    let weapon = image.find(&WEAPON)?;
    let perk = image.find(&PERK)?;
    let vehicle_refresh = refresh_of(&vehicle, VEHICLE)?;

    image.expect(
        "weapon bind script call",
        weapon + SCRIPT_CALL.0,
        SCRIPT_CALL.1,
    )?;
    let script = image
        .branch_target(weapon + SCRIPT_CALL.0 + 3)
        .ok_or("weapon bind has no script call")?;
    let training_key = image
        .branch_target(perk + TRAINING_KEY_CALL)
        .ok_or("perk bind has no training key call")?;
    image.expect(
        "weapon bind context service",
        weapon + CONTEXT_SERVICE.0,
        CONTEXT_SERVICE.1,
    )?;
    let context_service = image
        .u32(weapon + CONTEXT_SERVICE.0 + CONTEXT_SERVICE.1.len())
        .ok_or("context service offset out of range")? as usize;
    image.expect(
        "upgrade key load",
        vehicle_refresh + UPGRADE_KEY,
        UPGRADE_KEY_LOAD,
    )?;
    image.expect(
        "upgrade key add",
        vehicle_refresh + UPGRADE_KEY + 7,
        UPGRADE_KEY_ADD,
    )?;
    let upgrade_key = image
        .rip_target(
            vehicle_refresh + UPGRADE_KEY + 3,
            vehicle_refresh + UPGRADE_KEY + 7,
        )
        .ok_or("upgrade key out of range")?;
    let perk_limit_at = image.find(&PERK_LIMIT)?;
    let perk_limit = image.u32(perk_limit_at).ok_or("perk limit out of range")? as usize;
    for check in [&PANEL_LOCKS, &PANEL_MAY_PICK] {
        let at = image.find(check)?;
        if !(perk_limit_at..perk_limit_at + PERK_REFRESH_SPAN).contains(&at) {
            return Err(format!(
                "{} at {at:#x} is outside the perk refresh",
                check.name
            ));
        }
    }
    let fits = image.find(&VEHICLE_FITS)?;
    let walk = image
        .image
        .get(fits..fits + VEHICLE_FITS_SPAN)
        .ok_or("vehicle fit walk out of range")?;
    for list in 1..9u32 {
        let mut load = b"\x4c\x8b\xa0".to_vec();
        load.extend_from_slice(&(0x198 + 0x18 * list).to_le_bytes());
        load.extend_from_slice(b"\x4c\x8b\xb0");
        load.extend_from_slice(&(0x190 + 0x18 * list).to_le_bytes());
        if walk
            .windows(load.len())
            .filter(|w| *w == load.as_slice())
            .count()
            != 1
        {
            return Err(format!("vehicle fit list {list} is not where FITS expects"));
        }
    }
    if walk
        .windows(FIT_ENTRY_NAME.len())
        .filter(|w| *w == FIT_ENTRY_NAME)
        .count()
        != 9
    {
        return Err("vehicle fit entries are not named at +0x28".into());
    }
    if image.branch_target(image.call(&TRAINING_ROWS)?) != Some(training_key) {
        return Err("training table walk uses another key".into());
    }
    let fit = image.find(&SQUAD_FIT_NAME)?;
    let squad_fit_name = image.u32(fit).ok_or("squad fit name out of range")? as usize;
    if image.u32(fit - 10) != Some(squad_fit_name as u32 + 0x10) {
        return Err("squad fit name and size disagree".into());
    }

    let sites = Sites {
        training_show: image.find(&TRAINING_SHOW)?,
        training_layout: image.find(&TRAINING_LAYOUT)?,
        dispatch: image.find(&DISPATCH)?,
        destroy: image.find(&DESTROY)?,
        ammo: image.find(&AMMO)?,
        weapon,
        script,
        listen: image.find(&LISTEN)?,
        thumb: image.find(&THUMB)?,
        slider_dispatch: image.find(&SLIDER_DISPATCH)?,
        perk,
        upgrade: image.find(&UPGRADE)?,
        training_destroy: slot(&training, TRAINING, 0)?,
        vehicle_destroy: slot(&vehicle, VEHICLE, 0)?,
        vehicle_refresh,
        refresh: refresh_of(&panel, PANEL)?,
        training_key,
        squad_chooser: image.find(&SQUAD_CHOOSER)?,
        vehicle_chooser: image.find(&VEHICLE_CHOOSER)?,
        training_vtable: training.methods_at,
        panel_vtable: panel.methods_at,
        vehicle_vtable: vehicle.methods_at,
        slider_vtable: slider.methods_at,
        slider_ctrl_vtable: slider_ctrl.methods_at,
        upgrade_key,
        context_service,
        perk_limit,
        squad_fit_name,
    };

    // The vtable slots the plugin replaces hold the functions it chains to.
    for (what, found, expected) in [
        (
            "TrainingWindow destructor",
            sites.training_destroy,
            image.find(&TRAINING_DESTROY)?,
        ),
        (
            "vehicle panel destructor",
            sites.vehicle_destroy,
            image.find(&VEHICLE_DESTROY)?,
        ),
        (
            "TrainingWindow dispatch",
            slot(&training, TRAINING, 1)?,
            sites.dispatch,
        ),
        (
            "squad panel destructor",
            slot(&panel, PANEL, 0)?,
            sites.destroy,
        ),
        (
            "squad panel dispatch",
            slot(&panel, PANEL, 1)?,
            sites.dispatch,
        ),
        (
            "vehicle panel dispatch",
            slot(&vehicle, VEHICLE, 1)?,
            sites.dispatch,
        ),
        (
            "slider dispatch",
            slot(&slider_ctrl, SLIDER_CTRL, 1)?,
            sites.slider_dispatch,
        ),
    ] {
        if found != expected {
            return Err(format!("{what} is at {found:#x}, not {expected:#x}"));
        }
    }

    image.expect("script lookup entry", sites.script, SCRIPT_PREFIX)?;
    image.expect(
        "training key entry",
        sites.training_key,
        TRAINING_KEY_PREFIX,
    )?;
    image.expect(
        "TrainingWindow show entry",
        sites.training_show,
        TRAINING_SHOW_PREFIX,
    )?;
    if !REFRESH_PREFIXES
        .iter()
        .any(|prefix| image.starts_with(sites.refresh, prefix))
    {
        return Err(format!(
            "squad refresh entry at {:#x} starts differently",
            sites.refresh
        ));
    }
    image.expect(
        "vehicle refresh entry",
        sites.vehicle_refresh,
        VEHICLE_REFRESH_PREFIX,
    )?;
    image.expect("slider thumb entry", sites.thumb, THUMB_PREFIX)?;
    image.expect(
        "squad chooser entry",
        sites.squad_chooser,
        SQUAD_CHOOSER_PREFIX,
    )?;
    image.expect(
        "vehicle chooser entry",
        sites.vehicle_chooser,
        VEHICLE_CHOOSER_PREFIX,
    )?;
    Ok(sites)
}

impl Sites {
    /// The hooked entries, as (rva, relocated length, name).
    pub(crate) fn hooks(&self) -> [(usize, usize, &'static str); 6] {
        [
            (
                self.training_show,
                TRAINING_SHOW_PREFIX.len(),
                "training show",
            ),
            (self.refresh, REFRESH_PREFIXES[0].len(), "refresh"),
            (
                self.vehicle_refresh,
                VEHICLE_REFRESH_PREFIX.len(),
                "vehicle refresh",
            ),
            (self.thumb, THUMB_PREFIX.len(), "thumb layout"),
            (
                self.squad_chooser,
                SQUAD_CHOOSER_PREFIX.len(),
                "squad chooser",
            ),
            (
                self.vehicle_chooser,
                VEHICLE_CHOOSER_PREFIX.len(),
                "vehicle chooser",
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    /// The rvas and offsets the plugin's per-build table held before it
    /// resolved them, in [`BUILDS`] order.
    const TABLE: [(&str, Sites); 6] = [
        (
            "gog/2025-12-23",
            Sites {
                training_show: 0x299360,
                training_layout: 0x299dc0,
                dispatch: 0x2c2da0,
                destroy: 0x29c4e0,
                ammo: 0x3cb00,
                weapon: 0x3674b0,
                script: 0x709e0,
                listen: 0x2d3270,
                thumb: 0x2c6180,
                slider_dispatch: 0x2c52d0,
                perk: 0x2cff30,
                upgrade: 0x365740,
                training_destroy: 0x2990a0,
                vehicle_destroy: 0x2a25c0,
                vehicle_refresh: 0x2a3c40,
                refresh: 0x29e6c0,
                training_key: 0x298950,
                squad_chooser: 0x29cc40,
                vehicle_chooser: 0x2a2ed0,
                training_vtable: 0x518368,
                panel_vtable: 0x5187c8,
                vehicle_vtable: 0x518b28,
                slider_vtable: 0x51a1e0,
                slider_ctrl_vtable: 0x51a1b0,
                upgrade_key: 0x6080e8,
                context_service: 0x118,
                perk_limit: 0x878,
                squad_fit_name: 0x850,
            },
        ),
        (
            "steam/2025-12-23",
            Sites {
                training_show: 0x29e6f0,
                training_layout: 0x29f150,
                dispatch: 0x2c8130,
                destroy: 0x2a1870,
                ammo: 0x3cb00,
                weapon: 0x36d960,
                script: 0x70a60,
                listen: 0x2d8600,
                thumb: 0x2cb510,
                slider_dispatch: 0x2ca660,
                perk: 0x2d52c0,
                upgrade: 0x36bbf0,
                training_destroy: 0x29e430,
                vehicle_destroy: 0x2a7950,
                vehicle_refresh: 0x2a8fd0,
                refresh: 0x2a3a50,
                training_key: 0x29dce0,
                squad_chooser: 0x2a1fd0,
                vehicle_chooser: 0x2a8260,
                training_vtable: 0x51ea50,
                panel_vtable: 0x51eea8,
                vehicle_vtable: 0x51f208,
                slider_vtable: 0x5208c0,
                slider_ctrl_vtable: 0x520890,
                upgrade_key: 0x60f748,
                context_service: 0x138,
                perk_limit: 0x878,
                squad_fit_name: 0x850,
            },
        ),
        (
            "gog/2026-09-14",
            Sites {
                training_show: 0x29b220,
                training_layout: 0x29bd30,
                dispatch: 0x2c4e20,
                destroy: 0x29e500,
                ammo: 0x3cca0,
                weapon: 0x3698c0,
                script: 0x70b80,
                listen: 0x2d5400,
                thumb: 0x2c8200,
                slider_dispatch: 0x2c7350,
                perk: 0x2d20c0,
                upgrade: 0x367b50,
                training_destroy: 0x29af60,
                vehicle_destroy: 0x2a4640,
                vehicle_refresh: 0x2a5cc0,
                refresh: 0x2a08a0,
                training_key: 0x29a810,
                squad_chooser: 0x29ec60,
                vehicle_chooser: 0x2a4f50,
                training_vtable: 0x51a4a0,
                panel_vtable: 0x51a908,
                vehicle_vtable: 0x51ac58,
                slider_vtable: 0x51c310,
                slider_ctrl_vtable: 0x51c2e0,
                upgrade_key: 0x60a228,
                context_service: 0x118,
                perk_limit: 0x880,
                squad_fit_name: 0x860,
            },
        ),
        (
            "steam/2026-09-22",
            Sites {
                training_show: 0x2a05e0,
                training_layout: 0x2a10f0,
                dispatch: 0x2ca1e0,
                destroy: 0x2a38c0,
                ammo: 0x3cca0,
                weapon: 0x36fda0,
                script: 0x70c00,
                listen: 0x2da7c0,
                thumb: 0x2cd5c0,
                slider_dispatch: 0x2cc710,
                perk: 0x2d7480,
                upgrade: 0x36e030,
                training_destroy: 0x2a0320,
                vehicle_destroy: 0x2a9a00,
                vehicle_refresh: 0x2ab080,
                refresh: 0x2a5c60,
                training_key: 0x29fbd0,
                squad_chooser: 0x2a4020,
                vehicle_chooser: 0x2aa310,
                training_vtable: 0x521bb0,
                panel_vtable: 0x522018,
                vehicle_vtable: 0x522368,
                slider_vtable: 0x523a20,
                slider_ctrl_vtable: 0x5239f0,
                upgrade_key: 0x613848,
                context_service: 0x138,
                perk_limit: 0x880,
                squad_fit_name: 0x860,
            },
        ),
        (
            "gog/2026-09-25",
            Sites {
                training_show: 0x29b220,
                training_layout: 0x29bd30,
                dispatch: 0x2c4e20,
                destroy: 0x29e500,
                ammo: 0x3cca0,
                weapon: 0x3698d0,
                script: 0x70b80,
                listen: 0x2d5400,
                thumb: 0x2c8200,
                slider_dispatch: 0x2c7350,
                perk: 0x2d20c0,
                upgrade: 0x367b60,
                training_destroy: 0x29af60,
                vehicle_destroy: 0x2a4640,
                vehicle_refresh: 0x2a5cc0,
                refresh: 0x2a08a0,
                training_key: 0x29a810,
                squad_chooser: 0x29ec60,
                vehicle_chooser: 0x2a4f50,
                training_vtable: 0x51a4a0,
                panel_vtable: 0x51a908,
                vehicle_vtable: 0x51ac58,
                slider_vtable: 0x51c310,
                slider_ctrl_vtable: 0x51c2e0,
                upgrade_key: 0x60a228,
                context_service: 0x118,
                perk_limit: 0x880,
                squad_fit_name: 0x860,
            },
        ),
        (
            "steam/2026-09-25",
            Sites {
                training_show: 0x2a05e0,
                training_layout: 0x2a10f0,
                dispatch: 0x2ca1e0,
                destroy: 0x2a38c0,
                ammo: 0x3cca0,
                weapon: 0x36fdb0,
                script: 0x70c00,
                listen: 0x2da7c0,
                thumb: 0x2cd5c0,
                slider_dispatch: 0x2cc710,
                perk: 0x2d7480,
                upgrade: 0x36e040,
                training_destroy: 0x2a0320,
                vehicle_destroy: 0x2a9a00,
                vehicle_refresh: 0x2ab080,
                refresh: 0x2a5c60,
                training_key: 0x29fbd0,
                squad_chooser: 0x2a4020,
                vehicle_chooser: 0x2aa310,
                training_vtable: 0x521bb0,
                panel_vtable: 0x522018,
                vehicle_vtable: 0x522368,
                slider_vtable: 0x523a20,
                slider_ctrl_vtable: 0x5239f0,
                upgrade_key: 0x613848,
                context_service: 0x138,
                perk_limit: 0x880,
                squad_fit_name: 0x860,
            },
        ),
    ];

    #[test]
    fn sites_resolve_where_the_build_table_had_them() {
        for (build, expected) in TABLE {
            assert!(BUILDS.contains(&build), "{build}");
            let Some(mapped) = reference(build, "game.dll") else {
                eprintln!("skipping {build}: no bin/{build}/game.dll");
                continue;
            };
            assert_eq!(sites(&Image::mapped(&mapped)), Ok(expected), "{build}");
        }
    }

    #[test]
    fn sites_resolve_on_every_build() {
        for build in BUILDS {
            if let Some(mapped) = reference(build, "game.dll") {
                assert!(sites(&Image::mapped(&mapped)).is_ok(), "{build}");
            }
        }
    }

    /// Every hook relocates whole instructions that need no fixup.
    #[test]
    fn displaced_prologues_need_no_relocation() {
        for build in BUILDS {
            let Some(mapped) = reference(build, "game.dll") else {
                continue;
            };
            let sites = sites(&Image::mapped(&mapped)).unwrap();
            for (rva, len, name) in sites.hooks() {
                let before = &mapped.image[rva..rva + len];
                assert!(len >= 14, "{build} {name}");
                assert!(
                    defiance_core::decode::validate_copy(before).is_ok(),
                    "{build} {name}"
                );
            }
        }
    }

    #[test]
    fn a_changed_entry_is_refused() {
        let (build, expected) = TABLE[0];
        let Some(mapped) = reference(build, "game.dll") else {
            return;
        };
        let mut changed = mapped.image.clone();
        // The vehicle refresh is found through its vtable, not a signature,
        // so only the entry check catches this.
        changed[expected.vehicle_refresh + 1] ^= 0x01;
        let image = Image {
            image: &changed,
            base: mapped.base,
        };
        assert!(sites(&image).is_err());
    }
}
