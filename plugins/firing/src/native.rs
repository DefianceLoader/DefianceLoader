//! Only ABI adapters and service calls live here; raw layouts live in Core.
use crate::model::{self, Member};
use core::ffi::c_void;
use defiance_api::{GameAccessV1, MemberStateV1};
use std::sync::OnceLock;
pub static GAME: OnceLock<&'static GameAccessV1> = OnceLock::new();

unsafe fn snapshot(ai: *mut c_void) -> Option<Vec<MemberStateV1>> {
    let game = GAME.get()?;
    let entity = unsafe { (game.entity_from_facet)(ai) };
    let own = unsafe { (game.selectable)(entity) };
    let entities = unsafe { defiance_feature_sdk::services::members(game, entity) }?;
    let count = entities.len();
    let mut members = Vec::new();
    members.try_reserve_exact(count).ok()?;
    for entity in entities {
        let mut state = MemberStateV1::default();
        if unsafe { (game.read_member)(entity, own, &mut state) } == 0
            && state.enabled != 0
            && state.squad_selectable == own
        {
            members.push(state);
        }
    }
    Some(members)
}
fn decisions(members: &[MemberStateV1]) -> Vec<Member> {
    members
        .iter()
        .map(|m| Member {
            selected: m.selected != 0,
            pin: m.firing_pin,
        })
        .collect()
}
pub unsafe extern "C" fn set(ai: *mut c_void, value: u8) {
    let Some(game) = GAME.get() else {
        return;
    };
    if let Some(members) = unsafe { snapshot(ai) } {
        let subset = model::discriminates(&decisions(&members));
        for m in &members {
            if !subset || m.selected != 0 {
                unsafe { (game.set_firing_pin)(m.selectable, value, u8::from(subset)) };
            }
        }
        if subset {
            return;
        }
    }
    unsafe { (game.set_squad_firing)(ai, value) };
}
pub unsafe extern "C" fn ui(ai: *mut c_void) -> u8 {
    let Some(game) = GAME.get() else {
        return 0;
    };
    let flag = unsafe { (game.squad_firing)(ai) };
    match unsafe { snapshot(ai) } {
        Some(members) => model::ui(&decisions(&members), flag),
        None => flag,
    }
}

#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn defiance_test_firing_api(game: *const GameAccessV1) -> i32 {
    if game.is_null() {
        return 1;
    }
    if GAME.set(unsafe { &*game }).is_ok() {
        0
    } else {
        1
    }
}
#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn defiance_test_firing_set(ai: *mut c_void, value: u8) {
    unsafe { set(ai, value) }
}
#[cfg(feature = "parity-test")]
#[no_mangle]
pub unsafe extern "C" fn defiance_test_firing_ui(ai: *mut c_void) -> u8 {
    unsafe { ui(ai) }
}
