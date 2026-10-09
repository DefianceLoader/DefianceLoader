; Command-local member filtering for attack orders. Runs AFTER native
; collection, BEFORE per-human order creation. No persistent squad membership
; or order ownership is changed.
;
; attack: logic+43bab9, rbp-10 = copied members, r13 = count, inside fn_43af70.
; The entry is a CALL (rsp 8 mod 16). Its member filter is
; patch/order-selected.asm, assembled after this file.

attack_members:
    sub rsp, 0x28
    lea rcx, [rbp - 0x10]
    mov rdx, r13
    call selected_members
    lea rcx, [rbp - 0x10]
    mov rdx, rax
    mov r8, qword ptr [rsp + 0x80]    ; the caller's [rsp+50]: the order handle
    call capable_members
    mov r13, rax
    add rsp, 0x28
    mov rax, qword ptr [rsp + 0x58]   ; displaced mov rax, [rsp+50]
    ret

; rcx members, rdx count, r8 the order handle's address; rax the kept count.
; Keep only members that can attack the target with enabled ammunition (AI
; ai_can_attack(kind, 1), the cursor's own test before it offers attack). A
; soldier whose every weapon is disabled would otherwise fire the round
; already chambered, then stop. A ground target (kind 0x800) keeps the
; dispatcher's own gunner check. The order's target is read as the
; dispatcher reads it: handle -> order +10 -> target +28 -> +10.
capable_members:
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    push r14
    sub rsp, 0x28
    mov rsi, rcx
    mov r12, rdx
    mov rax, rdx
    mov rcx, qword ptr [r8]
    test rcx, rcx
    jz capable_done
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz capable_done
    mov rcx, qword ptr [rcx + 0x28]
    test rcx, rcx
    jz capable_done
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz capable_done
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x68]    ; vt:logic/HumanDamageableFacet@Leonardo|logic/DamageableModulesFacet@Leonardo|logic/DamageableBuildingFacet@Leonardo
    mov r14d, eax
    mov rax, r12
    cmp r14d, 0x800
    je capable_done
    xor ebx, ebx
    xor edi, edi
capable_next:
    cmp rbx, r12
    jae capable_packed
    mov r13, qword ptr [rsi + rbx*8]
    inc rbx
    mov rcx, r13
    test rcx, rcx
    jz capable_keep                   ; no member or AI: the dispatcher's own
    mov rax, qword ptr [rcx]          ; checks decide
    call qword ptr [rax + 0xb0]    ; vt:essence/EntityImpl@Essence@Galileo
    test rax, rax
    jz capable_keep
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz capable_keep
    mov rax, qword ptr [rcx]
    mov edx, r14d
    mov r8b, 1
    call qword ptr [rax + {ai_can_attack}]    ; vt:logic/SquadAiFacet@Leonardo
    test al, al
    jz capable_next
capable_keep:
    mov qword ptr [rsi + rdi*8], r13
    inc rdi
    jmp capable_next
capable_packed:
    mov rax, rdi
capable_done:
    add rsp, 0x28
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret
