//! Only the contract is shared as source. The counter storage lives in provider.dll.
//! Service: provider `example.counter`, name `counter`, exact version 1.
//! Both operations are thread-safe. State lasts until process exit; no allocations
//! or game pointers cross the boundary. Increment wraps at u64::MAX.
#[repr(C)]
pub struct CounterV1 {
    pub get: unsafe extern "C" fn() -> u64,
    pub increment: unsafe extern "C" fn() -> u64,
}
