//! Core's `selection-snapshot` service: copies of the player's current
//! selection and of the command targets in it, as logic.dll's own gatherer
//! picks them.
//!
//! The native gatherer (`(player, out vector)`) asks the player for its team
//! (vtable +0x40), looks up the team's selection manager, asks the manager for
//! its entity vector (vtable +0x58) and pushes, in the vector's order, each
//! entity that passes two gates:
//!
//! 1. selected: the entity's facet record (vtable +0xb0) holds a selectable
//!    facet (+0x50) whose is-selected query (vtable +0x58) answers yes;
//! 2. commandable: the record's control facet (+0x20) answers its query
//!    (vtable +0x80), or, for an entity without one, the entity answers its
//!    flag query (vtable +0x98) for flag 0x200.
//!
//! [`resolve`] finds the gatherer by its whole body, which pins every offset
//! above, and takes the manager lookup from its call. The table reproduces the
//! walk with the null checks the native code leaves out, copying into a local
//! list first so one call answers from one pass over the vector.
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use defiance_api::SelectionSnapshotV1;
use defiance_core::sites::Image;

/// The gatherer's body, up to the end of its loop.
const GATHERER: &str = "48895c2420564883ec20488b01488bdaff5040488bc8e8????????\
    488bc8488b10ff5258488b4b10488bf0482b0b48c1f903488b5008482b1048c1fa03483bd1\
    761b48b8ffffffffffffff1f483bd00f87????????488bcbe8????????48896c2438488b6e08\
    488b36483bf50f84????????48897c2440660f1f840000000000488b3e488bcf48897c2430\
    488b07ff90b0000000488b48504885c97465488b01ff505884c0745b488b07488bcfff90b000\
    0000488b48204885c9740b488b01ff9080000000eb164885ff7436488b07ba00020000488bcf\
    ff909800000084c07421488b5308483b5310740a48893a4883430808eb0d4c8d442430488bcb\
    e8????????4883c608483bf50f85????????";
/// The gatherer's call of the manager lookup.
const LOOKUP_CALL: usize = 0x16;
/// The lookup's start: `push rbx; sub rsp, 0x20`, then the thread-local
/// block (`mov rax, gs:[0x58]`) its static manager cache is guarded by.
const LOOKUP_PROLOGUE: [u8; 15] = [
    0x40, 0x53, 0x48, 0x83, 0xec, 0x20, 0x65, 0x48, 0x8b, 0x04, 0x25, 0x58, 0x00, 0x00, 0x00,
];

const PLAYER_TEAM: usize = 0x40;
const MANAGER_ENTITIES: usize = 0x58;
const ENTITY_FACETS: usize = 0xb0;
const RECORD_SELECTABLE: usize = 0x50;
const IS_SELECTED: usize = 0x58;
const RECORD_CONTROL: usize = 0x20;
const CONTROL_COMMANDABLE: usize = 0x80;
const ENTITY_FLAG: usize = 0x98;
const COMMANDABLE_FLAG: u32 = 0x200;
/// More entities than any selection holds: a larger vector is not one.
const MAX_ENTITIES: usize = 100_000;

/// The rva of the build's manager lookup.
pub(crate) fn resolve(image: &Image) -> Result<usize, String> {
    let pattern = defiance_core::pattern::parse(GATHERER)?;
    match defiance_core::scan::scan(image.image, &pattern)[..] {
        [gatherer] => {
            let lookup = image
                .branch_target(gatherer + LOOKUP_CALL)
                .ok_or("the selection gatherer calls no manager lookup")?;
            image.expect("selection manager lookup", lookup, &LOOKUP_PROLOGUE)?;
            Ok(lookup)
        }
        [] => Err("no selection gatherer".into()),
        ref found => Err(format!("{} selection gatherers", found.len())),
    }
}

/// [`resolve`] on the loaded logic.dll, read from the original view, and
/// the table armed with the lookup's address.
pub(crate) fn resolve_loaded(api: &defiance_api::Api) -> Result<(), String> {
    let base = unsafe { (api.module_base)(c"logic.dll".as_ptr()) } as *const u8;
    if base.is_null() {
        return Err("logic.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base.cast_mut().cast()) };
    let image = crate::plan::original_image(base, size).ok_or("logic.dll is unreadable")?;
    let lookup = resolve(&Image {
        image: &image,
        base: base as usize,
    })?;
    LOOKUP.store(base as usize + lookup, Ordering::Release);
    Ok(())
}

/// The native manager lookup's address; zero until resolved.
static LOOKUP: AtomicUsize = AtomicUsize::new(0);

unsafe fn method(object: *mut c_void, offset: usize) -> usize {
    unsafe { *(*object.cast::<*const usize>()).add(offset / 8) }
}

/// The object's argumentless virtual call at `offset`, or null for a null object.
unsafe fn get(object: *mut c_void, offset: usize) -> *mut c_void {
    if object.is_null() {
        return core::ptr::null_mut();
    }
    let call: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
        unsafe { core::mem::transmute(method(object, offset)) };
    unsafe { call(object) }
}

unsafe fn ask(object: *mut c_void, offset: usize) -> bool {
    let call: unsafe extern "C" fn(*mut c_void) -> u8 =
        unsafe { core::mem::transmute(method(object, offset)) };
    unsafe { call(object) != 0 }
}

unsafe fn field(object: *mut c_void, offset: usize) -> *mut c_void {
    unsafe { *object.cast::<u8>().add(offset).cast::<*mut c_void>() }
}

/// Gate 1: the entity's selectable facet says it is selected.
unsafe fn selected(entity: *mut c_void) -> bool {
    let record = unsafe { get(entity, ENTITY_FACETS) };
    if record.is_null() {
        return false;
    }
    let selectable = unsafe { field(record, RECORD_SELECTABLE) };
    !selectable.is_null() && unsafe { ask(selectable, IS_SELECTED) }
}

/// Gate 2, for an entity that passed gate 1.
unsafe fn commandable(entity: *mut c_void) -> bool {
    let record = unsafe { get(entity, ENTITY_FACETS) };
    if record.is_null() {
        return false;
    }
    let control = unsafe { field(record, RECORD_CONTROL) };
    if !control.is_null() {
        return unsafe { ask(control, CONTROL_COMMANDABLE) };
    }
    let flag: unsafe extern "C" fn(*mut c_void, u32) -> u8 =
        unsafe { core::mem::transmute(method(entity, ENTITY_FLAG)) };
    unsafe { flag(entity, COMMANDABLE_FLAG) != 0 }
}

/// The player's selection-manager entities, or None when Core has no lookup,
/// the player, team or manager is missing, or the vector is malformed.
unsafe fn entities(player: *mut c_void) -> Option<Vec<*mut c_void>> {
    let lookup = LOOKUP.load(Ordering::Acquire);
    if lookup == 0 {
        return None;
    }
    let team = unsafe { get(player, PLAYER_TEAM) };
    if team.is_null() {
        return None;
    }
    let lookup: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
        unsafe { core::mem::transmute(lookup) };
    let manager = unsafe { lookup(team) };
    let vector = unsafe { get(manager, MANAGER_ENTITIES) };
    if vector.is_null() {
        return None;
    }
    let begin = unsafe { field(vector, 0) } as usize;
    let end = unsafe { field(vector, 8) } as usize;
    let bytes = end.checked_sub(begin)?;
    if bytes % 8 != 0 || begin % 8 != 0 || (begin == 0 && bytes != 0) {
        return None;
    }
    if bytes / 8 > MAX_ENTITIES {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(begin as *const *mut c_void, bytes / 8) }.to_vec())
}

/// Copies `found` under the count/capacity contract.
unsafe fn deliver(found: &[*mut c_void], out: *mut *mut c_void, capacity: usize) -> usize {
    let count = found.len();
    if count != 0 && capacity >= count {
        if out.is_null() {
            return usize::MAX;
        }
        unsafe { core::ptr::copy_nonoverlapping(found.as_ptr(), out, count) };
    }
    count
}

unsafe extern "C" fn copy_selected(
    player: *mut c_void,
    out: *mut *mut c_void,
    capacity: usize,
) -> usize {
    let Some(mut found) = (unsafe { entities(player) }) else {
        return usize::MAX;
    };
    found.retain(|&entity| !entity.is_null() && unsafe { selected(entity) });
    unsafe { deliver(&found, out, capacity) }
}

unsafe extern "C" fn copy_command_targets(
    player: *mut c_void,
    out: *mut *mut c_void,
    capacity: usize,
) -> usize {
    let Some(mut found) = (unsafe { entities(player) }) else {
        return usize::MAX;
    };
    found.retain(|&entity| {
        !entity.is_null() && unsafe { selected(entity) } && unsafe { commandable(entity) }
    });
    unsafe { deliver(&found, out, capacity) }
}

pub static API: SelectionSnapshotV1 = SelectionSnapshotV1 {
    copy_selected,
    copy_command_targets,
};

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    /// An object whose first word is its vtable.
    #[repr(C)]
    struct Object {
        vtable: *const usize,
        fields: [usize; 12],
    }

    fn object(vtable: &[usize]) -> Box<Object> {
        Box::new(Object {
            vtable: vtable.as_ptr(),
            fields: [0; 12],
        })
    }

    fn set(object: &mut Object, offset: usize, value: usize) {
        object.fields[offset / 8 - 1] = value;
    }

    unsafe extern "C" fn first_field(object: *mut c_void) -> *mut c_void {
        unsafe { field(object, 8) }
    }
    /// The answer a fake holds in its second field.
    unsafe extern "C" fn answer(object: *mut c_void) -> u8 {
        unsafe { field(object, 16) as usize as u8 }
    }
    unsafe extern "C" fn flag(object: *mut c_void, flag: u32) -> u8 {
        u8::from(flag == COMMANDABLE_FLAG && unsafe { field(object, 16) } as usize != 0)
    }
    /// The manager lookup: the team's first field.
    unsafe extern "C" fn lookup(team: *mut c_void) -> *mut c_void {
        unsafe { field(team, 8) }
    }

    struct World {
        vtables: [Vec<usize>; 2],
        objects: Vec<Box<Object>>,
        vector: Box<[usize; 2]>,
        list: Vec<usize>,
        player: *mut c_void,
        manager: usize,
    }

    /// Entity `i` is selected when `selected[i]`; it is commandable through a
    /// control facet when `control[i]` is Some, else through its flag answer
    /// `flagged[i]`. A zero entry in `list` stays a null entity.
    fn world(selected: &[bool], control: &[Option<bool>], flagged: &[bool]) -> World {
        let mut vtable = vec![0usize; 32];
        vtable[PLAYER_TEAM / 8] = first_field as *const () as usize;
        vtable[ENTITY_FACETS / 8] = first_field as *const () as usize;
        vtable[IS_SELECTED / 8] = answer as *const () as usize;
        vtable[CONTROL_COMMANDABLE / 8] = answer as *const () as usize;
        vtable[ENTITY_FLAG / 8] = flag as *const () as usize;
        let mut objects = Vec::new();
        let mut list = Vec::new();
        for i in 0..selected.len() {
            let mut selectable = object(&vtable);
            set(&mut selectable, 16, usize::from(selected[i]));
            let mut record = object(&vtable);
            set(
                &mut record,
                RECORD_SELECTABLE,
                &*selectable as *const _ as usize,
            );
            if let Some(answer) = control[i] {
                let mut control = object(&vtable);
                set(&mut control, 16, usize::from(answer));
                set(&mut record, RECORD_CONTROL, &*control as *const _ as usize);
                objects.push(control);
            }
            let mut entity = object(&vtable);
            set(&mut entity, 8, &*record as *const _ as usize);
            set(&mut entity, 16, usize::from(flagged[i]));
            list.push(&*entity as *const _ as usize);
            objects.extend([selectable, record, entity]);
        }
        let vector = Box::new([
            list.as_ptr() as usize,
            list.as_ptr() as usize + list.len() * 8,
        ]);
        // The manager's +0x58 answers its first field: the vector.
        let mut manager_vtable = vec![0usize; 32];
        manager_vtable[MANAGER_ENTITIES / 8] = first_field as *const () as usize;
        let mut manager = object(&manager_vtable);
        set(&mut manager, 8, &*vector as *const _ as usize);
        let mut team = object(&vtable);
        set(&mut team, 8, &*manager as *const _ as usize);
        let mut player = object(&vtable);
        set(&mut player, 8, &*team as *const _ as usize);
        let player_ptr = &*player as *const _ as *mut c_void;
        let manager_ptr = &*manager as *const _ as usize;
        objects.extend([manager, team, player]);
        LOOKUP.store(lookup as *const () as usize, Ordering::Release);
        World {
            vtables: [vtable, manager_vtable],
            objects,
            vector,
            list,
            player: player_ptr,
            manager: manager_ptr,
        }
    }

    // One test owns LOOKUP's fake so parallel tests never see it unset.
    #[test]
    fn snapshots_follow_the_gates_order_and_capacity_contract() {
        let mut w = world(
            &[true, false, true, true],
            &[Some(true), Some(true), Some(false), None],
            &[false, true, true, true],
        );
        let e = |i: usize| w.list[i] as *mut c_void;
        let mut out = [9usize as *mut c_void; 4];
        unsafe {
            assert_eq!(copy_selected(w.player, core::ptr::null_mut(), 0), 3);
            assert_eq!(copy_selected(w.player, out.as_mut_ptr(), 2), 3);
            assert_eq!(
                out, [9usize as *mut c_void; 4],
                "a short buffer is untouched"
            );
            assert_eq!(copy_selected(w.player, out.as_mut_ptr(), 4), 3);
            assert_eq!(out[..3], [e(0), e(2), e(3)]);
            assert_eq!(copy_command_targets(w.player, out.as_mut_ptr(), 4), 2);
            assert_eq!(out[..2], [e(0), e(3)]);
            assert_eq!(
                copy_selected(w.player, core::ptr::null_mut(), 4),
                usize::MAX
            );

            assert_eq!(
                copy_selected(core::ptr::null_mut(), out.as_mut_ptr(), 4),
                usize::MAX
            );
            // A null entity in the vector is skipped, not dereferenced.
            w.list[1] = 0;
            assert_eq!(copy_selected(w.player, out.as_mut_ptr(), 4), 3);
            // Malformed vectors: misaligned end, end before begin, too large.
            w.vector[1] = w.vector[0] + 3;
            assert_eq!(copy_selected(w.player, out.as_mut_ptr(), 4), usize::MAX);
            w.vector[1] = w.vector[0] - 8;
            assert_eq!(
                copy_command_targets(w.player, out.as_mut_ptr(), 4),
                usize::MAX
            );
            w.vector[1] = w.vector[0] + (MAX_ENTITIES + 1) * 8;
            assert_eq!(copy_selected(w.player, out.as_mut_ptr(), 4), usize::MAX);
            // An empty selection is a count of zero.
            w.vector[1] = w.vector[0];
            assert_eq!(copy_selected(w.player, core::ptr::null_mut(), 0), 0);
            // A missing manager or vector.
            set(&mut *(w.manager as *mut Object), 8, 0);
            assert_eq!(copy_selected(w.player, out.as_mut_ptr(), 4), usize::MAX);
            // Without a resolved lookup nothing is answered.
            LOOKUP.store(0, Ordering::Release);
            assert_eq!(copy_selected(w.player, out.as_mut_ptr(), 4), usize::MAX);
        }
        drop((w.vtables, w.objects));
    }

    /// The manager lookup's rva, per build.
    const LOOKUP_RVA: [(&str, usize); 8] = [
        ("gog/2025-12-23", 0x52e930),
        ("steam/2025-12-23", 0x52e9c0),
        ("gog/2026-09-14", 0x541cd0),
        ("steam/2026-09-22", 0x541d60),
        ("gog/2026-09-25", 0x542450),
        ("steam/2026-09-25", 0x5424e0),
        ("gog/2026-10-07", 0x546a20),
        ("steam/2026-10-07", 0x546ab0),
    ];

    #[test]
    fn the_lookup_resolves_on_every_build_in_bin() {
        assert_eq!(LOOKUP_RVA.map(|(build, _)| build), BUILDS);
        for (build, rva) in LOOKUP_RVA {
            let Some(mapped) = reference(build, "logic.dll") else {
                continue;
            };
            assert_eq!(resolve(&Image::mapped(&mapped)), Ok(rva), "{build}");
        }
    }

    #[test]
    fn a_changed_gate_is_refused() {
        let Some(mapped) = reference(LOOKUP_RVA[0].0, "logic.dll") else {
            return;
        };
        let pattern = defiance_core::pattern::parse(GATHERER).unwrap();
        let [gatherer] = defiance_core::scan::scan(&mapped.image, &pattern)[..] else {
            panic!("one gatherer");
        };
        // The team, entity-vector, record, selectable, is-selected, control,
        // commandable, flag-query and flag offsets, and the lookup call.
        for at in [18, LOOKUP_CALL + 1, 35, 144, 151, 162, 182, 193, 209, 217] {
            let mut changed = mapped.image.clone();
            changed[gatherer + at] ^= 8;
            let image = Image {
                image: &changed,
                base: mapped.base,
            };
            assert!(resolve(&image).is_err(), "byte {at}");
        }
    }
}
