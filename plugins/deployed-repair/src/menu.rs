//! Preserve the existing repair permission while applying deployment restrictions.
use std::sync::atomic::AtomicUsize;

pub(crate) static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub(crate) static RESUME: AtomicUsize = AtomicUsize::new(0);
pub(crate) static HELICOPTER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub(crate) static HELICOPTER_RESUME: AtomicUsize = AtomicUsize::new(0);
pub(crate) static HELICOPTER_QUERY_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Reproduce the native virtual call, then clear its helicopter veto only for
/// an active landed state. +0x68 queries the top state, unlike +0x70's whole
/// stack. The native branch and remaining crew checks consume the result.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn helicopter() {
    core::arch::naked_asm!(
        "mov r11, qword ptr [rip+{slot}]",
        "call qword ptr [rax+r11]",
        "test al, al",
        "jz 3f",
        "push rax",
        "push rcx",
        "push rdx",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "sub rsp, 0x88",
        "movdqu [rsp+0x20], xmm0",
        "movdqu [rsp+0x30], xmm1",
        "movdqu [rsp+0x40], xmm2",
        "movdqu [rsp+0x50], xmm3",
        "movdqu [rsp+0x60], xmm4",
        "movdqu [rsp+0x70], xmm5",
        "mov rax, [r13]",
        "mov rcx, r13",
        "mov edx, 0x13",
        "call qword ptr [rax+0x68]",
        "test al, al",
        "jz 2f",
        "mov byte ptr [rsp+0xb8], 0",
        "2:",
        "movdqu xmm0, [rsp+0x20]",
        "movdqu xmm1, [rsp+0x30]",
        "movdqu xmm2, [rsp+0x40]",
        "movdqu xmm3, [rsp+0x50]",
        "movdqu xmm4, [rsp+0x60]",
        "movdqu xmm5, [rsp+0x70]",
        "lea rsp, [rsp+0x88]",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "3:",
        "jmp qword ptr [rip+{resume}]",
        resume = sym HELICOPTER_RESUME,
        slot = sym HELICOPTER_QUERY_SLOT,
    );
}

/// The signature fixes R13 as the AI facet and EDI as the permission mask.
/// The native site has an aligned call stack. Preserve all volatile registers
/// and XMM registers across the extra state query, then reproduce AND's flags.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn mask() {
    core::arch::naked_asm!(
        "push rax",
        "push rcx",
        "push rdx",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "sub rsp, 0x88",
        "movdqu [rsp+0x20], xmm0",
        "movdqu [rsp+0x30], xmm1",
        "movdqu [rsp+0x40], xmm2",
        "movdqu [rsp+0x50], xmm3",
        "movdqu [rsp+0x60], xmm4",
        "movdqu [rsp+0x70], xmm5",
        "mov rax, [r13]",
        "mov rcx, r13",
        "mov edx, 0x1a",
        "call qword ptr [rax+0x70]",
        "test al, al",
        "jnz 2f",
        "and edi, 0xfff765ba",
        "jmp 3f",
        "2:",
        "and edi, 0xfff7e5ba",
        "3:",
        "movdqu xmm0, [rsp+0x20]",
        "movdqu xmm1, [rsp+0x30]",
        "movdqu xmm2, [rsp+0x40]",
        "movdqu xmm3, [rsp+0x50]",
        "movdqu xmm4, [rsp+0x60]",
        "movdqu xmm5, [rsp+0x70]",
        "lea rsp, [rsp+0x88]",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "jmp qword ptr [rip+{resume}]",
        resume = sym RESUME,
    );
}

/// Bind the native fixture continuation and expose the shipping detour.
#[cfg(feature = "parity-test")]
#[no_mangle]
pub extern "C" fn deployed_repair_test_menu_detour(resume: usize) -> usize {
    RESUME.store(resume, std::sync::atomic::Ordering::Release);
    mask as *const () as usize
}

#[cfg(feature = "parity-test")]
#[no_mangle]
pub extern "C" fn deployed_repair_test_helicopter_detour(resume: usize, slot: usize) -> usize {
    HELICOPTER_RESUME.store(resume, std::sync::atomic::Ordering::Release);
    HELICOPTER_QUERY_SLOT.store(slot, std::sync::atomic::Ordering::Release);
    helicopter as *const () as usize
}
