; Boarding fills the first free dummy mount. Rebalance after a passenger enters
; or leaves so a gun with a special weapon type takes precedence over a usual
; gun when fewer mounts than passengers are available.
vehicle_special_fire_board_tail:
    mov rcx, rbp
    push rax
    sub rsp, 0x28
    call vehicle_special_fire_rebalance
    add rsp, 0x28
    pop rax
    mov rbx, qword ptr [rsp + 0x78]
    jmp {vehicle_special_fire_board_resume}

vehicle_special_fire_disembark_tail:
    mov rcx, rbp
    push rax
    sub rsp, 0x28
    call vehicle_special_fire_rebalance
    add rsp, 0x28
    pop rax
    mov rsi, qword ptr [rsp + 0x48]
    jmp {vehicle_special_fire_disembark_resume}

vehicle_special_fire_rebalance:
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x50
    mov rbx, rcx
    mov rcx, qword ptr [rbx + 0x20]
    test rcx, rcx
    jz vehicle_special_fire_rebalance_done
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    mov r12, qword ptr [rax + 0x28]
    test r12, r12
    jz vehicle_special_fire_rebalance_done
    mov rcx, rbx
    mov rdx, r12
    call vehicle_special_fire_cleanup
    mov r15d, 1
vehicle_special_fire_pass:
    mov rsi, qword ptr [rbx + 0x110]
    mov rdi, qword ptr [rbx + 0x118]
vehicle_special_fire_passenger:
    cmp rsi, rdi
    jae vehicle_special_fire_next_pass
    mov rcx, qword ptr [rsi]
    test rcx, rcx
    jz vehicle_special_fire_next_passenger
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    mov rax, qword ptr [rax + 0x28]
    test rax, rax
    jz vehicle_special_fire_next_passenger
    mov rcx, rax
    call {vehicle_special_fire_source}
    mov qword ptr [rsp + 0x20], rax
    test rax, rax
    jz vehicle_special_fire_next_passenger
    mov rcx, qword ptr [rax + 0x40]
    test rcx, rcx
    jz vehicle_special_fire_next_passenger
    xor eax, eax
    cmp dword ptr [rcx + 0x108], 0
    setne al
    cmp eax, r15d
    jne vehicle_special_fire_next_passenger
    mov rdx, qword ptr [rsp + 0x20]
    call vehicle_special_fire_is_bound
    test al, al
    jne vehicle_special_fire_next_passenger
    mov r13, qword ptr [r12 + 0x208]
    mov r14, qword ptr [r12 + 0x210]
vehicle_special_fire_try_free:
    cmp r13, r14
    jae vehicle_special_fire_try_preempt
    mov rcx, qword ptr [r13]
    mov rax, qword ptr [rcx]
    mov rdx, qword ptr [rsp + 0x20]
    call qword ptr [rax + 0x148]
    test al, al
    jne vehicle_special_fire_bound
    add r13, 8
    jmp vehicle_special_fire_try_free
vehicle_special_fire_try_preempt:
    test r15d, r15d
    jz vehicle_special_fire_next_passenger
    mov r13, qword ptr [r12 + 0x208]
    mov r14, qword ptr [r12 + 0x210]
vehicle_special_fire_next_gunner:
    cmp r13, r14
    jae vehicle_special_fire_next_passenger
    mov rcx, qword ptr [r13]
    mov qword ptr [rsp + 0x30], rcx
    mov r8, qword ptr [rcx + 0x68]
    mov r9, qword ptr [rcx + 0x70]
vehicle_special_fire_scan_bound:
    cmp r8, r9
    jae vehicle_special_fire_advance_gunner
    mov rax, qword ptr [r8]
    test rax, rax
    jz vehicle_special_fire_advance_bound
    mov rcx, qword ptr [rsp + 0x20]
    mov rdx, qword ptr [rax]
    cmp rdx, qword ptr [rcx]
    jne vehicle_special_fire_advance_bound
    mov rdx, qword ptr [rax + 0x40]
    test rdx, rdx
    jz vehicle_special_fire_advance_bound
    cmp dword ptr [rdx + 0x108], 0
    jne vehicle_special_fire_advance_bound
    mov qword ptr [rsp + 0x28], rax
    mov rcx, qword ptr [rsp + 0x30]
    mov rax, qword ptr [rcx]
    mov rdx, qword ptr [rsp + 0x28]
    call qword ptr [rax + 0x150]
    test al, al
    je vehicle_special_fire_advance_gunner
    mov rcx, qword ptr [rsp + 0x30]
    mov rax, qword ptr [rcx]
    mov rdx, qword ptr [rsp + 0x20]
    call qword ptr [rax + 0x148]
    test al, al
    jne vehicle_special_fire_bound
    mov rcx, qword ptr [rsp + 0x30]
    mov rax, qword ptr [rcx]
    mov rdx, qword ptr [rsp + 0x28]
    call qword ptr [rax + 0x148]
    jmp vehicle_special_fire_advance_gunner
vehicle_special_fire_advance_bound:
    add r8, 8
    jmp vehicle_special_fire_scan_bound
vehicle_special_fire_advance_gunner:
    add r13, 8
    jmp vehicle_special_fire_next_gunner
vehicle_special_fire_bound:
    mov rcx, r12
    call {vehicle_special_fire_refresh_ai}
vehicle_special_fire_next_passenger:
    add rsi, 8
    jmp vehicle_special_fire_passenger
vehicle_special_fire_next_pass:
    test r15d, r15d
    jz vehicle_special_fire_rebalance_done
    xor r15d, r15d
    jmp vehicle_special_fire_pass
vehicle_special_fire_rebalance_done:
    add rsp, 0x50
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret

vehicle_special_fire_is_bound:
    mov r8, qword ptr [r12 + 0x208]
    mov r9, qword ptr [r12 + 0x210]
vehicle_special_fire_bound_gunner:
    cmp r8, r9
    jae vehicle_special_fire_not_bound
    mov r10, qword ptr [r8]
    mov r11, qword ptr [r10 + 0x68]
    mov r10, qword ptr [r10 + 0x70]
vehicle_special_fire_bound_slot:
    cmp r11, r10
    jae vehicle_special_fire_bound_next
    cmp qword ptr [r11], rdx
    je vehicle_special_fire_already_bound
    add r11, 8
    jmp vehicle_special_fire_bound_slot
vehicle_special_fire_bound_next:
    add r8, 8
    jmp vehicle_special_fire_bound_gunner
vehicle_special_fire_already_bound:
    mov eax, 1
    ret
vehicle_special_fire_not_bound:
    xor eax, eax
    ret

; Retain one binding per source gun, and only while its owner remains aboard.
; Null unbinding discards ghost copies without restoring duplicated ammunition
; into a gun that may already have fired on foot. Refresh the vehicle model
; after removal using the native server model refresh path.
vehicle_special_fire_cleanup:
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x30
    mov rbx, rcx
    mov r12, rdx
    mov dword ptr [rsp + 0x20], 0
    mov r13, qword ptr [r12 + 0x208]
    mov r14, qword ptr [r12 + 0x210]
vehicle_special_fire_cleanup_gunner:
    cmp r13, r14
    jae vehicle_special_fire_cleanup_done
    mov rcx, qword ptr [r13]
    mov qword ptr [rsp + 0x28], rcx
    mov rax, qword ptr [rcx + 0x50]
    cmp rax, qword ptr [rcx + 0x58]
    jne vehicle_special_fire_cleanup_next_gunner
    mov rsi, qword ptr [rcx + 0x70]
    sub rsi, qword ptr [rcx + 0x68]
    cmp rsi, 512
    ja vehicle_special_fire_cleanup_next_gunner
    test rsi, 7
    jne vehicle_special_fire_cleanup_next_gunner
    mov rax, qword ptr [rcx + 0x28]
    sub rax, qword ptr [rcx + 0x20]
    cmp rax, rsi
    jne vehicle_special_fire_cleanup_next_gunner
    xor r15d, r15d
vehicle_special_fire_cleanup_slot:
    cmp r15, rsi
    jae vehicle_special_fire_cleanup_next_gunner
    mov rax, qword ptr [rsp + 0x28]
    mov rdi, qword ptr [rax + 0x68]
    add rdi, r15
    mov rdx, qword ptr [rdi]
    test rdx, rdx
    jz vehicle_special_fire_cleanup_next_slot
    mov rcx, rbx
    call vehicle_special_fire_roster_gun
    test al, al
    jz vehicle_special_fire_cleanup_release
    mov rcx, r12
    mov rdx, qword ptr [rdi]
    mov r8, rdi
    call vehicle_special_fire_duplicate
    test al, al
    jz vehicle_special_fire_cleanup_next_slot
vehicle_special_fire_cleanup_release:
    mov rax, qword ptr [rsp + 0x28]
    mov rcx, qword ptr [rax + 0x20]
    mov rcx, qword ptr [rcx + r15]
    test rcx, rcx
    jz vehicle_special_fire_cleanup_clear
    mov rax, qword ptr [rcx]
    xor edx, edx
    call qword ptr [rax + 0x60]
vehicle_special_fire_cleanup_clear:
    mov qword ptr [rdi], 0
    mov dword ptr [rsp + 0x20], 1
vehicle_special_fire_cleanup_next_slot:
    add r15, 8
    jmp vehicle_special_fire_cleanup_slot
vehicle_special_fire_cleanup_next_gunner:
    add r13, 8
    jmp vehicle_special_fire_cleanup_gunner
vehicle_special_fire_cleanup_done:
    cmp dword ptr [rsp + 0x20], 0
    je vehicle_special_fire_cleanup_return
    mov rcx, r12
    call {vehicle_special_fire_refresh_ai}
vehicle_special_fire_cleanup_return:
    add rsp, 0x30
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret

; Pointer membership in every passenger's complete gun list also retains a
; legitimate primary binding when the preferred special gun changes.
vehicle_special_fire_roster_gun:
    push rbx
    push rsi
    push rdi
    sub rsp, 0x20
    mov rbx, rdx
    mov rsi, qword ptr [rcx + 0x110]
    mov rdi, qword ptr [rcx + 0x118]
vehicle_special_fire_roster_passenger:
    cmp rsi, rdi
    jae vehicle_special_fire_roster_missing
    mov rcx, qword ptr [rsi]
    test rcx, rcx
    jz vehicle_special_fire_roster_next
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    mov rax, qword ptr [rax + 0x28]
    test rax, rax
    jz vehicle_special_fire_roster_next
    mov rax, qword ptr [rax + 0x1f0]
    test rax, rax
    jz vehicle_special_fire_roster_next
    mov rax, qword ptr [rax + 0x10]
    test rax, rax
    jz vehicle_special_fire_roster_next
    mov r8, qword ptr [rax + 0x38]
    mov r9, qword ptr [rax + 0x40]
    mov rax, r9
    sub rax, r8
    cmp rax, 512
    ja vehicle_special_fire_roster_next
    test rax, 7
    jne vehicle_special_fire_roster_next
vehicle_special_fire_roster_weapon:
    cmp r8, r9
    jae vehicle_special_fire_roster_next
    cmp qword ptr [r8], rbx
    je vehicle_special_fire_roster_found
    add r8, 8
    jmp vehicle_special_fire_roster_weapon
vehicle_special_fire_roster_next:
    add rsi, 8
    jmp vehicle_special_fire_roster_passenger
vehicle_special_fire_roster_missing:
    xor eax, eax
    jmp vehicle_special_fire_roster_return
vehicle_special_fire_roster_found:
    mov eax, 1
vehicle_special_fire_roster_return:
    add rsp, 0x20
    pop rdi
    pop rsi
    pop rbx
    ret

; Earlier bindings are canonical, including bindings in another gunner.
vehicle_special_fire_duplicate:
    push rbx
    mov r9, qword ptr [rcx + 0x208]
    mov r10, qword ptr [rcx + 0x210]
vehicle_special_fire_duplicate_gunner:
    cmp r9, r10
    jae vehicle_special_fire_duplicate_missing
    mov rax, qword ptr [r9]
    mov rcx, qword ptr [rax + 0x50]
    cmp rcx, qword ptr [rax + 0x58]
    jne vehicle_special_fire_duplicate_next
    mov r11, qword ptr [rax + 0x68]
    mov rcx, qword ptr [rax + 0x70]
    mov rbx, rcx
    sub rbx, r11
    cmp rbx, 512
    ja vehicle_special_fire_duplicate_next
    test rbx, 7
    jne vehicle_special_fire_duplicate_next
    mov rcx, qword ptr [rax + 0x28]
    sub rcx, qword ptr [rax + 0x20]
    cmp rcx, rbx
    jne vehicle_special_fire_duplicate_next
    add rcx, r11
vehicle_special_fire_duplicate_slot:
    cmp r11, r8
    je vehicle_special_fire_duplicate_missing
    cmp r11, rcx
    jae vehicle_special_fire_duplicate_next
    cmp qword ptr [r11], rdx
    je vehicle_special_fire_duplicate_found
    add r11, 8
    jmp vehicle_special_fire_duplicate_slot
vehicle_special_fire_duplicate_next:
    add r9, 8
    jmp vehicle_special_fire_duplicate_gunner
vehicle_special_fire_duplicate_missing:
    xor eax, eax
    pop rbx
    ret
vehicle_special_fire_duplicate_found:
    mov eax, 1
    pop rbx
    ret
