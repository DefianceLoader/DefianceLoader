//! Installed capacity, shared across DLLs through Core, never through Rust statics
//! linked separately into consumers. Zero means no provider has published yet.
use std::sync::atomic::{AtomicU32, Ordering};
static SLOTS: AtomicU32 = AtomicU32::new(0);
fn publish_into(state: &AtomicU32, slots: u32) -> i32 {
    if !(9..=126).contains(&slots) || slots % 3 != 0 {
        return 1;
    }
    match state.compare_exchange(0, slots, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => 0,
        Err(_) => 2,
    }
}
unsafe extern "C" fn capacity() -> u32 {
    SLOTS.load(Ordering::Acquire).max(9)
}
unsafe extern "C" fn publish(slots: u32) -> i32 {
    publish_into(&SLOTS, slots)
}
pub static API: defiance_api::AmmoMenuV1 = defiance_api::AmmoMenuV1 { capacity, publish };

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_is_validated_and_one_shot_even_for_stock_capacity() {
        for slots in [9, 12, 36, 126] {
            let state = AtomicU32::new(0);
            assert_eq!(state.load(Ordering::Acquire).max(9), 9);
            for invalid in [0, 8, 10, 127, u32::MAX] {
                assert_eq!(publish_into(&state, invalid), 1);
                assert_eq!(state.load(Ordering::Acquire), 0);
            }
            assert_eq!(publish_into(&state, slots), 0);
            assert_eq!(publish_into(&state, 36), 2);
            assert_eq!(state.load(Ordering::Acquire), slots);
        }
    }
}
