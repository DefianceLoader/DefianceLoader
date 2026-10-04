; The native one-second mount update can move into empty positions. Exchange
; a blocked special gun with an ordinary gun that can aim at the same target.
; Transfer the fields owned by the source gun, including ammunition and reload
; state, while keeping each mount's geometry and target. Owning handles and
; magazine maps move between the two live dummies without changing refcounts.
vehicle_special_fire_dynamic_tick:
    mov dword ptr [rbx + 0xbc], 0x3f800000
    push rax
    push rcx
    push rdx
    push r8
    push r9
    push r10
    push r11
    sub rsp, 0x28
    mov rcx, rbx
    call vehicle_special_fire_dynamic_rebalance
    test al, al
    lea rsp, [rsp + 0x28]
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdx
    pop rcx
    pop rax
    jne vehicle_special_fire_dynamic_skip
    jmp {vehicle_special_fire_dynamic_resume}
vehicle_special_fire_dynamic_skip:
    jmp {vehicle_special_fire_guard_skip}

vehicle_special_fire_dynamic_rebalance:
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x30
    mov rbx, rcx
    mov rax, qword ptr [rbx + 0x50]
    cmp rax, qword ptr [rbx + 0x58]
    jne vehicle_special_fire_dynamic_done
    mov rax, qword ptr [rbx + 0x28]
    sub rax, qword ptr [rbx + 0x20]
    mov rdx, qword ptr [rbx + 0x70]
    sub rdx, qword ptr [rbx + 0x68]
    cmp rax, rdx
    jne vehicle_special_fire_dynamic_done
    cmp rax, 16
    jb vehicle_special_fire_dynamic_done
    mov rax, qword ptr [rbx + 0x98]
    test rax, rax
    jz vehicle_special_fire_dynamic_done
    cmp qword ptr [rax + 0x10], 0
    je vehicle_special_fire_dynamic_done
    xor esi, esi
vehicle_special_fire_dynamic_special:
    mov rax, qword ptr [rbx + 0x68]
    lea rdx, [rax + rsi]
    cmp rdx, qword ptr [rbx + 0x70]
    jae vehicle_special_fire_dynamic_done
    mov r12, qword ptr [rdx]
    mov rax, qword ptr [rbx + 0x20]
    mov r14, qword ptr [rax + rsi]
    mov rcx, r14
    mov rdx, r12
    call vehicle_special_fire_dynamic_live
    test al, al
    jz vehicle_special_fire_dynamic_next_special
    mov rax, qword ptr [r12 + 0x40]
    cmp dword ptr [rax + 0x108], 0
    je vehicle_special_fire_dynamic_next_special
    cmp byte ptr [rax + 0xa9], 0
    jne vehicle_special_fire_dynamic_next_special
    cmp dword ptr [r14 + 0xdc], 0
    jle vehicle_special_fire_dynamic_next_special
    cmp byte ptr [r14 + 0xe2], 0
    je vehicle_special_fire_dynamic_next_special
    mov rcx, r14
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x1c0]
    test al, al
    jne vehicle_special_fire_dynamic_next_special
    xor edi, edi
vehicle_special_fire_dynamic_destination:
    mov rax, qword ptr [rbx + 0x68]
    lea rdx, [rax + rdi]
    cmp rdx, qword ptr [rbx + 0x70]
    jae vehicle_special_fire_dynamic_next_special
    mov r13, qword ptr [rdx]
    mov rax, qword ptr [rbx + 0x20]
    mov r15, qword ptr [rax + rdi]
    mov rcx, r15
    mov rdx, r13
    call vehicle_special_fire_dynamic_live
    test al, al
    jz vehicle_special_fire_dynamic_next_destination
    mov rax, qword ptr [r13 + 0x40]
    cmp dword ptr [rax + 0x108], 0
    jne vehicle_special_fire_dynamic_next_destination
    mov rcx, r15
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x1c0]
    test al, al
    jz vehicle_special_fire_dynamic_next_destination
    mov eax, dword ptr [r14 + 0x140]
    mov dword ptr [rsp + 0x20], eax
    mov eax, dword ptr [r15 + 0x140]
    mov dword ptr [rsp + 0x24], eax
    call vehicle_special_fire_dynamic_exchange
    mov rcx, r15
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x1c0]
    test al, al
    jz vehicle_special_fire_dynamic_restore
    mov rax, qword ptr [rbx + 0x68]
    mov qword ptr [rax + rsi], r13
    mov qword ptr [rax + rdi], r12
    mov rax, rdi
    shr rax, 3
    mov dword ptr [rbx + 0x158], eax
    mov rax, rsi
    shr rax, 3
    mov dword ptr [rbx + 0x15c], eax
    or dword ptr [rbx + 0x13c], 0x1000
    mov eax, 1
    jmp vehicle_special_fire_dynamic_return
vehicle_special_fire_dynamic_restore:
    call vehicle_special_fire_dynamic_exchange
    mov eax, dword ptr [rsp + 0x20]
    mov dword ptr [r14 + 0x140], eax
    mov eax, dword ptr [rsp + 0x24]
    mov dword ptr [r15 + 0x140], eax
    jmp vehicle_special_fire_dynamic_done
vehicle_special_fire_dynamic_next_destination:
    add rdi, 8
    jmp vehicle_special_fire_dynamic_destination
vehicle_special_fire_dynamic_next_special:
    add rsi, 8
    jmp vehicle_special_fire_dynamic_special
vehicle_special_fire_dynamic_done:
    xor eax, eax
vehicle_special_fire_dynamic_return:
    add rsp, 0x30
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret

; Only live bindings that share the gunner's current target can be exchanged.
; A matching descriptor also excludes an unbound dummy or a mismatched source.
vehicle_special_fire_dynamic_live:
    xor eax, eax
    test rdx, rdx
    jz vehicle_special_fire_dynamic_live_done
    test rcx, rcx
    jz vehicle_special_fire_dynamic_live_done
    mov r8, qword ptr [rdx]
    cmp r8, qword ptr [rcx]
    jne vehicle_special_fire_dynamic_live_done
    mov r8, qword ptr [rdx + 0x40]
    test r8, r8
    jz vehicle_special_fire_dynamic_live_done
    cmp r8, qword ptr [rcx + 0x40]
    jne vehicle_special_fire_dynamic_live_done
    mov r8, qword ptr [rbx + 0x98]
    cmp r8, qword ptr [rcx + 0xc8]
    jne vehicle_special_fire_dynamic_live_done
    mov eax, 1
vehicle_special_fire_dynamic_live_done:
    ret

vehicle_special_fire_dynamic_exchange:
    mov rax, qword ptr [r14 + 0x20]
    mov rdx, qword ptr [r15 + 0x20]
    mov qword ptr [r14 + 0x20], rdx
    mov qword ptr [r15 + 0x20], rax
    mov rax, qword ptr [r14 + 0x40]
    mov rdx, qword ptr [r15 + 0x40]
    mov qword ptr [r14 + 0x40], rdx
    mov qword ptr [r15 + 0x40], rax
    mov rax, qword ptr [r14 + 0x50]
    mov rdx, qword ptr [r15 + 0x50]
    mov qword ptr [r14 + 0x50], rdx
    mov qword ptr [r15 + 0x50], rax
    mov rax, qword ptr [r14 + 0x58]
    mov rdx, qword ptr [r15 + 0x58]
    mov qword ptr [r14 + 0x58], rdx
    mov qword ptr [r15 + 0x58], rax
    lea r8, [r14 + 0xd4]
    lea r9, [r15 + 0xd4]
    mov ecx, 5
vehicle_special_fire_dynamic_state:
    mov rax, qword ptr [r8]
    mov rdx, qword ptr [r9]
    mov qword ptr [r8], rdx
    mov qword ptr [r9], rax
    add r8, 8
    add r9, 8
    dec ecx
    jne vehicle_special_fire_dynamic_state
    mov eax, dword ptr [r8]
    mov edx, dword ptr [r9]
    mov dword ptr [r8], edx
    mov dword ptr [r9], eax
    lea r8, [r14 + 0x110]
    lea r9, [r15 + 0x110]
    mov ecx, 2
vehicle_special_fire_dynamic_magazines:
    mov rax, qword ptr [r8]
    mov rdx, qword ptr [r9]
    mov qword ptr [r8], rdx
    mov qword ptr [r9], rax
    add r8, 8
    add r9, 8
    dec ecx
    jne vehicle_special_fire_dynamic_magazines
    mov al, byte ptr [r14 + 0x12d]
    mov dl, byte ptr [r15 + 0x12d]
    mov byte ptr [r14 + 0x12d], dl
    mov byte ptr [r15 + 0x12d], al
    mov ax, word ptr [r14 + 0x12e]
    mov dx, word ptr [r15 + 0x12e]
    mov word ptr [r14 + 0x12e], dx
    mov word ptr [r15 + 0x12e], ax
    mov al, byte ptr [r14 + 0x145]
    mov dl, byte ptr [r15 + 0x145]
    mov byte ptr [r14 + 0x145], dl
    mov byte ptr [r15 + 0x145], al
    sub rsp, 0x28
    mov rcx, r14
    call vehicle_special_fire_dynamic_target
    mov rcx, r15
    call vehicle_special_fire_dynamic_target
    add rsp, 0x28
    ret

; The target setter consumes a strong and auxiliary reference. It rebuilds
; ballistic context from the exchanged ammunition and resets the mount's aim
; controller; the direction getter alone cannot initialize a different gun.
vehicle_special_fire_dynamic_target:
    push rbx
    sub rsp, 0x30
    mov rbx, rcx
    mov rax, qword ptr [rbx + 0xc8]
    mov qword ptr [rsp + 0x20], rax
    test rax, rax
    jz vehicle_special_fire_dynamic_set_target
    inc dword ptr [rax + 8]
    cmp dword ptr [rax + 8], 1
    jne vehicle_special_fire_dynamic_target_aux
    mov rcx, rax
    mov rdx, qword ptr [rax]
    call qword ptr [rdx + 8]
    mov rax, qword ptr [rsp + 0x20]
vehicle_special_fire_dynamic_target_aux:
    cmp qword ptr [rax + 0x10], 0
    je vehicle_special_fire_dynamic_set_target
    inc dword ptr [rax + 0x18]
vehicle_special_fire_dynamic_set_target:
    mov rcx, rbx
    lea rdx, [rsp + 0x20]
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x30]
    add rsp, 0x30
    pop rbx
    ret

; The client consumes the native move event as a one-way move into a free
; mount. An occupied destination instead requires the same two-way exchange.
vehicle_special_fire_dynamic_client:
    push rax
    push rcx
    push rdx
    push r8
    push r9
    push r10
    push r11
    sub rsp, 0x28
    mov rcx, rsi
    mov rdx, qword ptr [r15 + 0x60]
    call vehicle_special_fire_dynamic_client_swap
    test al, al
    lea rsp, [rsp + 0x28]
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdx
    pop rcx
    pop rax
    jne vehicle_special_fire_dynamic_client_skip
    mov rbx, qword ptr [r15 + 0x60]
    mov rax, rbx
    jmp {vehicle_special_fire_dynamic_client_resume}
vehicle_special_fire_dynamic_client_skip:
    jmp {vehicle_special_fire_dynamic_client_skip}

vehicle_special_fire_dynamic_client_swap:
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x20
    mov rbx, rcx
    mov edi, edx
    shr rdx, 32
    mov esi, edx
    cmp esi, edi
    je vehicle_special_fire_dynamic_client_handled
    mov rax, qword ptr [rbx + 0x20]
    sub rax, qword ptr [rbx + 0x18]
    mov rdx, qword ptr [rbx + 0x68]
    sub rdx, qword ptr [rbx + 0x60]
    cmp rax, rdx
    jne vehicle_special_fire_dynamic_client_handled
    shr rax, 3
    cmp rsi, rax
    jae vehicle_special_fire_dynamic_client_handled
    cmp rdi, rax
    jae vehicle_special_fire_dynamic_client_handled
    mov rax, qword ptr [rbx + 0x60]
    mov r12, qword ptr [rax + rsi*8]
    mov r13, qword ptr [rax + rdi*8]
    test r12, r12
    jz vehicle_special_fire_dynamic_client_handled
    mov rax, qword ptr [rbx + 0x18]
    mov r14, qword ptr [rax + rsi*8]
    mov r15, qword ptr [rax + rdi*8]
    test r14, r14
    jz vehicle_special_fire_dynamic_client_handled
    test r15, r15
    jz vehicle_special_fire_dynamic_client_handled
    mov rax, qword ptr [r12]
    cmp rax, qword ptr [r14]
    jne vehicle_special_fire_dynamic_client_handled
    test r13, r13
    jz vehicle_special_fire_dynamic_client_stock
    mov rax, qword ptr [r13]
    cmp rax, qword ptr [r15]
    jne vehicle_special_fire_dynamic_client_handled
    call vehicle_special_fire_dynamic_exchange
    mov rax, qword ptr [rbx + 0x60]
    mov qword ptr [rax + rsi*8], r13
    mov qword ptr [rax + rdi*8], r12
vehicle_special_fire_dynamic_client_handled:
    mov eax, 1
    jmp vehicle_special_fire_dynamic_client_return
vehicle_special_fire_dynamic_client_stock:
    xor eax, eax
vehicle_special_fire_dynamic_client_return:
    add rsp, 0x20
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret
