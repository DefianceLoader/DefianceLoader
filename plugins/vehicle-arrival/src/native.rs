//! Tighten only the ratio produced by the vehicle's stock arrival callback.
use core::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

pub(super) static ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
pub(super) static MULTIPLIER: AtomicU32 = AtomicU32::new(1.0f32.to_bits());

type Update = unsafe extern "C" fn(usize, usize);

pub(super) fn multiplier(percent: i64) -> Result<f32, &'static str> {
    if !(50..=100).contains(&percent) {
        return Err("braking_window_percent must be between 50 and 100");
    }
    Ok(100.0 / percent as f32)
}

/// The original fills all outputs first, so directions, maximum speed and
/// unrelated movement modes retain their native behavior. No persistent
/// state is kept; the host removes the owned hook at mission-free safe points.
#[cfg_attr(feature = "parity-test", no_mangle)]
pub unsafe extern "C" fn vehicle_arrival_update(chassis: usize, record: usize) {
    let original: Update = core::mem::transmute(ORIGINAL.load(Ordering::Acquire));
    original(chassis, record);
    tighten(
        chassis,
        record,
        f32::from_bits(MULTIPLIER.load(Ordering::Relaxed)),
    );
}

unsafe fn tighten(chassis: usize, record: usize, multiplier: f32) {
    if *((chassis + 0x320) as *const u32) != 1 || multiplier == 1.0 {
        return;
    }
    let ratio = *((record + 0x1d4) as *const f32);
    // The verified callback produces a fractional ratio only at a final
    // point. Empty paths and an exact endpoint produce zero; corners and
    // cruise produce one. Leave invalid native outputs untouched as well.
    if !(0.0 < ratio && ratio < 1.0) {
        return;
    }
    let next_ratio = (ratio * multiplier).min(1.0);
    let scale = next_ratio / ratio;
    let offsets = [0x1d0, 0x200, 0x204, 0x208];
    let values = offsets.map(|offset| *((record + offset) as *const f32));
    let next = values.map(|value| value * scale);
    if values[0] < 0.0 || !next.into_iter().all(f32::is_finite) {
        return;
    }
    *((record + 0x1d4) as *mut f32) = next_ratio;
    for (offset, value) in offsets.into_iter().zip(next) {
        *((record + offset) as *mut f32) = value;
    }
}

#[cfg(feature = "parity-test")]
#[no_mangle]
pub extern "C" fn vehicle_arrival_test_bind(original: *mut c_void, percent: i64) -> i32 {
    let Ok(multiplier) = multiplier(percent) else {
        return 1;
    };
    if original.is_null() {
        return 1;
    }
    MULTIPLIER.store(multiplier.to_bits(), Ordering::Relaxed);
    ORIGINAL.store(original, Ordering::Release);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_limits_the_requested_speed_boost() {
        assert_eq!(multiplier(50), Ok(2.0));
        assert_eq!(multiplier(100), Ok(1.0));
        assert!(multiplier(49).is_err());
        assert!(multiplier(101).is_err());
    }
}
