//! Core's `relation` service: whether the player owns an entity, and how the
//! player stands towards it, as logic.dll's `LogicUtilsImpl` answers both.
//!
//! Each `LogicUtilsImpl` relation query has the same body: it asks the entity
//! for its control (vtable +0xb0), reads the control's owner (+0x20) and
//! tail-calls one owner predicate (+0x80 the player's own, +0x88 ally, +0x90
//! enemy, +0x98 neutral, +0xa0 abandoned), answering false without an owner.
//! [`verify`] checks at preparation that the loaded build's vtable holds
//! exactly those five bodies in consecutive slots; the table then runs the
//! same chain itself, also checking the control the native code dereferences
//! unchecked. The queries sit one slot lower on the 2025 builds than on the
//! 2026 ones, so they are found by body, not by slot.
use core::ffi::c_void;
use defiance_api::{
    RelationV1, RELATION_ABANDONED, RELATION_ALLY, RELATION_ENEMY, RELATION_NEUTRAL, RELATION_NONE,
};
use defiance_core::sites::Image;

/// The class whose vtable holds the native relation queries.
const LOGIC_UTILS: &str = ".?AVLogicUtilsImpl@Leonardo@@";
/// The entity's control getter, as a vtable offset.
const ENTITY_CONTROL: usize = 0xb0;
/// The control's owner pointer.
const CONTROL_OWNER: usize = 0x20;
/// The owner's predicates, as vtable offsets, in slot order.
const OWN: u8 = 0x80;
const PREDICATES: [u8; 5] = [OWN, 0x88, 0x90, 0x98, 0xa0];
/// The predicates `relation` asks, in the order the native unit label asks
/// them, with the answer each gives.
const RELATIONS: [(u8, u32); 4] = [
    (0x88, RELATION_ALLY),
    (0x90, RELATION_ENEMY),
    (0x98, RELATION_NEUTRAL),
    (0xa0, RELATION_ABANDONED),
];

/// One relation query's body; [`PREDICATE_AT`] holds the owner predicate.
const QUERY: [u8; 51] = [
    0x48, 0x83, 0xec, 0x28, // sub rsp, 0x28
    0x48, 0x85, 0xd2, // test rdx, rdx
    0x74, 0x23, // jz none
    0x48, 0x8b, 0x02, // mov rax, [rdx]
    0x48, 0x8b, 0xca, // mov rcx, rdx
    0xff, 0x90, 0xb0, 0x00, 0x00, 0x00, // call [rax+0xb0]
    0x48, 0x8b, 0x48, 0x20, // mov rcx, [rax+0x20]
    0x48, 0x85, 0xc9, // test rcx, rcx
    0x74, 0x0e, // jz none
    0x48, 0x8b, 0x01, // mov rax, [rcx]
    0x48, 0x83, 0xc4, 0x28, // add rsp, 0x28
    0x48, 0xff, 0xa0, 0x00, 0x00, 0x00, 0x00, // jmp [rax+predicate]
    0x32, 0xc0, // none: xor al, al
    0x48, 0x83, 0xc4, 0x28, // add rsp, 0x28
    0xc3, // ret
];
const PREDICATE_AT: usize = 40;

/// The query body that tail-calls `predicate`.
fn query(predicate: u8) -> [u8; 51] {
    let mut body = QUERY;
    body[PREDICATE_AT] = predicate;
    body
}

/// Ok when the build's `LogicUtilsImpl` vtable holds the five relation
/// queries, each exactly once and in consecutive slots in [`PREDICATES`]
/// order, so the offsets the table reads are the ones the game reads.
pub(crate) fn verify(image: &Image) -> Result<(), String> {
    let vtable = image.vtable(LOGIC_UTILS)?;
    let mut slots = Vec::new();
    for predicate in PREDICATES {
        let body = query(predicate);
        let found: Vec<usize> = (0..vtable.methods.len())
            .filter(|&slot| image.starts_with(vtable.methods[slot], &body))
            .collect();
        match found[..] {
            [slot] => slots.push(slot),
            [] => return Err(format!("no relation query for owner +{predicate:#x}")),
            _ => {
                return Err(format!(
                    "{} relation queries for owner +{predicate:#x}",
                    found.len()
                ))
            }
        }
    }
    if slots.windows(2).any(|pair| pair[1] != pair[0] + 1) {
        return Err(format!(
            "relation queries in slots {slots:?}, not consecutive"
        ));
    }
    Ok(())
}

/// [`verify`] on the loaded logic.dll, read from the original view.
pub(crate) fn verify_loaded(api: &defiance_api::Api) -> Result<(), String> {
    let base = unsafe { (api.module_base)(c"logic.dll".as_ptr()) } as *const u8;
    if base.is_null() {
        return Err("logic.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base.cast_mut().cast()) };
    let image = crate::plan::original_image(base, size).ok_or("logic.dll is unreadable")?;
    verify(&Image {
        image: &image,
        base: base as usize,
    })
}

unsafe fn method(object: *mut c_void, offset: usize) -> usize {
    unsafe { *(*object.cast::<*const usize>()).add(offset / 8) }
}

/// The entity's owner, or null when it, its control or its owner is missing.
unsafe fn owner(entity: *mut c_void) -> *mut c_void {
    if entity.is_null() {
        return core::ptr::null_mut();
    }
    let control: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
        unsafe { core::mem::transmute(method(entity, ENTITY_CONTROL)) };
    let control = unsafe { control(entity) };
    if control.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        *control
            .cast::<u8>()
            .add(CONTROL_OWNER)
            .cast::<*mut c_void>()
    }
}

/// Whether the owner's predicate at `offset` holds.
unsafe fn holds(owner: *mut c_void, offset: u8) -> bool {
    let predicate: unsafe extern "C" fn(*mut c_void) -> u8 =
        unsafe { core::mem::transmute(method(owner, offset as usize)) };
    unsafe { predicate(owner) != 0 }
}

unsafe extern "C" fn owned(entity: *mut c_void) -> u8 {
    let owner = unsafe { owner(entity) };
    (!owner.is_null() && unsafe { holds(owner, OWN) }) as u8
}

unsafe extern "C" fn relation(entity: *mut c_void) -> u32 {
    let owner = unsafe { owner(entity) };
    if owner.is_null() {
        return RELATION_NONE;
    }
    RELATIONS
        .iter()
        .find(|(offset, _)| unsafe { holds(owner, *offset) })
        .map_or(RELATION_NONE, |&(_, relation)| relation)
}

pub static API: RelationV1 = RelationV1 { owned, relation };

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    /// An owner whose predicates answer from `answers`, one byte per
    /// [`PREDICATES`] entry.
    struct Owner {
        vtable: *const usize,
        answers: [u8; 5],
    }

    unsafe extern "C" fn answer<const I: usize>(owner: *mut c_void) -> u8 {
        unsafe { (*owner.cast::<Owner>()).answers[I] }
    }

    unsafe extern "C" fn control_of(entity: *mut c_void) -> *mut c_void {
        unsafe { *entity.cast::<*mut c_void>().add(1) }
    }

    fn owner_vtable() -> Vec<usize> {
        let mut vtable = vec![0usize; 32];
        let answers: [unsafe extern "C" fn(*mut c_void) -> u8; 5] = [
            answer::<0>,
            answer::<1>,
            answer::<2>,
            answer::<3>,
            answer::<4>,
        ];
        for (predicate, answer) in PREDICATES.iter().zip(answers) {
            vtable[*predicate as usize / 8] = answer as *const () as usize;
        }
        vtable
    }

    /// Asks both queries of an entity whose control holds `owner`.
    fn ask(owner: *mut c_void) -> (u8, u32) {
        let mut entity_vtable = vec![0usize; 32];
        entity_vtable[ENTITY_CONTROL / 8] = control_of as *const () as usize;
        let mut control = [0usize; 8];
        control[CONTROL_OWNER / 8] = owner as usize;
        let entity = [entity_vtable.as_ptr() as usize, control.as_ptr() as usize];
        let entity = entity.as_ptr() as *mut c_void;
        unsafe { (owned(entity), relation(entity)) }
    }

    #[test]
    fn queries_follow_the_owner_predicates_in_label_order() {
        let vtable = owner_vtable();
        let mut owner = Owner {
            vtable: vtable.as_ptr(),
            answers: [1, 0, 0, 0, 0],
        };
        let p = &mut owner as *mut Owner as *mut c_void;
        assert_eq!(ask(p), (1, RELATION_NONE));
        for (i, expected) in [
            RELATION_ALLY,
            RELATION_ENEMY,
            RELATION_NEUTRAL,
            RELATION_ABANDONED,
        ]
        .into_iter()
        .enumerate()
        {
            owner.answers = [0; 5];
            owner.answers[i + 1] = 1;
            assert_eq!(ask(p), (0, expected));
        }
        owner.answers = [0, 0, 1, 1, 1];
        assert_eq!(ask(p), (0, RELATION_ENEMY), "the first true predicate wins");
        assert!(!owner.vtable.is_null());
    }

    #[test]
    fn missing_entity_control_or_owner_is_no_relation() {
        unsafe {
            assert_eq!(owned(core::ptr::null_mut()), 0);
            assert_eq!(relation(core::ptr::null_mut()), RELATION_NONE);
        }
        assert_eq!(ask(core::ptr::null_mut()), (0, RELATION_NONE));
        unsafe extern "C" fn no_control(_: *mut c_void) -> *mut c_void {
            core::ptr::null_mut()
        }
        let mut entity_vtable = vec![0usize; 32];
        entity_vtable[ENTITY_CONTROL / 8] = no_control as *const () as usize;
        let entity = [entity_vtable.as_ptr() as usize];
        let entity = entity.as_ptr() as *mut c_void;
        unsafe {
            assert_eq!(owned(entity), 0);
            assert_eq!(relation(entity), RELATION_NONE);
        }
    }

    /// The `LogicUtilsImpl` slot holding the player's-own query, per build.
    const OWN_SLOT: [(&str, usize); 8] = [
        ("gog/2025-12-23", 195),
        ("steam/2025-12-23", 195),
        ("gog/2026-09-14", 196),
        ("steam/2026-09-22", 196),
        ("gog/2026-09-25", 196),
        ("steam/2026-09-25", 196),
        ("gog/2026-10-07", 196),
        ("steam/2026-10-07", 196),
    ];

    fn own_slot(image: &Image) -> usize {
        let vtable = image.vtable(LOGIC_UTILS).unwrap();
        (0..vtable.methods.len())
            .find(|&slot| image.starts_with(vtable.methods[slot], &query(OWN)))
            .unwrap()
    }

    #[test]
    fn the_queries_verify_on_every_build_in_bin() {
        assert_eq!(OWN_SLOT.map(|(build, _)| build), BUILDS);
        for (build, slot) in OWN_SLOT {
            let Some(mapped) = reference(build, "logic.dll") else {
                continue;
            };
            let image = Image::mapped(&mapped);
            assert_eq!(verify(&image), Ok(()), "{build}");
            assert_eq!(own_slot(&image), slot, "{build}");
        }
    }

    #[test]
    fn a_changed_query_body_is_refused() {
        let Some(mapped) = reference(OWN_SLOT[0].0, "logic.dll") else {
            return;
        };
        let image = Image::mapped(&mapped);
        let vtable = image.vtable(LOGIC_UTILS).unwrap();
        let ally = vtable.methods[own_slot(&image) + 1];
        for at in [17, 24, PREDICATE_AT] {
            let mut changed = mapped.image.clone();
            changed[ally + at] ^= 8;
            let image = Image {
                image: &changed,
                base: mapped.base,
            };
            assert!(verify(&image).is_err(), "byte {at}");
        }
    }
}
