//! Register adapter at the native attack-button continuation in game.dll.
use core::arch::naked_asm;
use std::sync::atomic::AtomicUsize;

pub(super) static ORIGINAL: AtomicUsize = AtomicUsize::new(0);

// The surrounding game function keeps its availability mask in EDI and its
// selected entity vector at RBP-0x49/-0x41 in every supported build. Preserve
// volatile registers, vector registers, flags, and the native frame while
// adding the attack bit for passengers whose ammo belongs to their soldiers.
#[unsafe(naked)]
pub(super) unsafe extern "system" fn availability() {
    naked_asm!(
        "pushfq", "push rax", "push rcx", "push rdx", "push r8", "push r9", "push r10", "push r11",
        "sub rsp, 0xa0",
        "movdqu [rsp + 0x20], xmm0", "movdqu [rsp + 0x30], xmm1", "movdqu [rsp + 0x40], xmm2",
        "movdqu [rsp + 0x50], xmm3", "movdqu [rsp + 0x60], xmm4", "movdqu [rsp + 0x70], xmm5",
        "mov [rsp + 0x80], edi",
        "mov rcx, [rbp - 0x49]", "mov rdx, [rbp - 0x41]",
        "call {available}", "test al, al", "jz 2f", "or dword ptr [rsp + 0x80], 2", "2:",
        "mov edi, [rsp + 0x80]",
        "movdqu xmm0, [rsp + 0x20]", "movdqu xmm1, [rsp + 0x30]", "movdqu xmm2, [rsp + 0x40]",
        "movdqu xmm3, [rsp + 0x50]", "movdqu xmm4, [rsp + 0x60]", "movdqu xmm5, [rsp + 0x70]",
        "add rsp, 0xa0",
        "pop r11", "pop r10", "pop r9", "pop r8", "pop rdx", "pop rcx", "pop rax", "popfq",
        "jmp qword ptr [rip + {original}]",
        available = sym crate::orders::available,
        original = sym ORIGINAL,
    );
}

#[cfg(test)]
pub(super) mod test_adapter {
    use super::*;

    #[unsafe(naked)]
    unsafe extern "system" fn continuation() {
        naked_asm!("ret");
    }

    // The real adapter is entered inside a function body with aligned RSP.
    // Supply that frame, then capture the volatile state after its continuation.
    #[unsafe(naked)]
    unsafe extern "system" fn invoke(frame: usize, output: *mut u64, mask: u32) -> u32 {
        naked_asm!(
            "push rbp", "push rdi", "push r12", "sub rsp, 0xb0",
            "mov rbp, rcx", "mov r12, rdx", "mov edi, r8d",
            "mov eax, 0x1111", "mov ecx, 0x2222", "mov edx, 0x3333",
            "mov r8d, 0x4444", "mov r9d, 0x5555", "mov r10d, 0x6666", "mov r11d, 0x7777",
            "pcmpeqd xmm0, xmm0", "movdqa xmm1, xmm0", "movdqa xmm2, xmm0",
            "movdqa xmm3, xmm0", "movdqa xmm4, xmm0", "movdqa xmm5, xmm0",
            "sub rsp, 8", "stc", "call {adapter}",
            "pushfq", "pop qword ptr [r12 + 56]", "add rsp, 8",
            "mov [r12], rax", "mov [r12 + 8], rcx", "mov [r12 + 16], rdx",
            "mov [r12 + 24], r8", "mov [r12 + 32], r9", "mov [r12 + 40], r10", "mov [r12 + 48], r11",
            "movdqu [r12 + 64], xmm0", "movdqu [r12 + 80], xmm1", "movdqu [r12 + 96], xmm2",
            "movdqu [r12 + 112], xmm3", "movdqu [r12 + 128], xmm4", "movdqu [r12 + 144], xmm5",
            "mov eax, edi", "add rsp, 0xb0", "pop r12", "pop rdi", "pop rbp", "ret",
            adapter = sym availability,
        );
    }

    pub(crate) unsafe fn check(begin: usize, end: usize, expected: u32) {
        ORIGINAL.store(
            continuation as *const () as usize,
            std::sync::atomic::Ordering::Release,
        );
        let mut frame = [0u8; 0x100];
        let rbp = unsafe { frame.as_mut_ptr().add(0x80) };
        unsafe {
            rbp.sub(0x49).cast::<usize>().write_unaligned(begin);
            rbp.sub(0x41).cast::<usize>().write_unaligned(end);
        }
        let mut state = [0u64; 20];
        let mask = unsafe { invoke(rbp as usize, state.as_mut_ptr(), 0x40) };
        assert_eq!(mask, expected);
        assert_eq!(
            &state[..7],
            &[0x1111, 0x2222, 0x3333, 0x4444, 0x5555, 0x6666, 0x7777]
        );
        assert_eq!(state[7] & 1, 1, "carry flag is preserved");
        assert!(state[8..].iter().all(|value| *value == u64::MAX));
    }
}
