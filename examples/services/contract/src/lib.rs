//! Only the contract is shared as source. The counter storage lives in provider.dll.
//! Service: provider `example.counter`, name `counter`, exact version 1.
//! Both operations are thread-safe. State lasts until process exit; no allocations
//! or game pointers cross the boundary. Increment wraps at u64::MAX.
#[repr(C)]
pub struct CounterV1 {
    pub get: unsafe extern "C" fn() -> u64,
    pub increment: unsafe extern "C" fn() -> u64,
}

/// Service: provider `example.counter-user`, name `total`, version 1; and
/// provider `example.counter-watch`, name `watch`, version 1. Each passes the
/// call on to the table it holds, down to the counter: a chain of cached
/// tables, which is what a hot reload must keep alive for a holder that stays.
#[repr(C)]
pub struct TotalV1 {
    pub total: unsafe extern "C" fn() -> u64,
}
