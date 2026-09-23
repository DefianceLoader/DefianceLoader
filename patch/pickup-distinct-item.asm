; Replacement recipient chooser for "pick up the weapon on the ground".
;
; The stock chooser is fn_43e0f0(squadHolder, slotTypeId, allowSwap). Its
; swap pass returns the first squad member whose held weapon has the wanted
; slot type, so a squad with two members of one type always hands the weapon
; to the lower-indexed member and the other can never be re-armed.
;
; This version keeps the stock behaviour except in that swap pass, where it
; prefers a matching member who is not already holding the very item being
; picked up. The first match is kept as a fallback, so when every matching
; member already holds that item the choice is the stock one.
;
; Called only from the pickup order site at 0x43b7d8. There the item info
; sits in r15 (set at 0x43b78b, read at 0x43b7c7, and non-volatile across
; the two intervening calls), so it is read here as a fourth argument.
;
;   rcx  SquadHolderFacet*   members at +0x1e8/+0x1f0, script info at +0x240,
;                            per-type slot counters at +0x260 (used) / +0x264 (max)
;   edx  slot type id
;   r8b  allowSwap
;   r15  InventoryItemScriptInfo* of the weapon on the ground
;
; HumanGunner member:  vt+0xc8 -> the man, vt+0x180 -> held slot type,
;                      +0x28 -> the gun, gun vt+0x160 -> its item info
; SquadScriptInfo:     +0x1a9 -> the noPickupGun perk (perk id 5)

    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    sub rsp, 0x20

    mov rbx, rcx
    movsxd r14, edx
    movzx r13d, r8b
    mov r12, r15
    xor ebp, ebp

    ; noPickupGun is a property of the squad type, so test it once
    mov rax, qword ptr [rbx + 0x240]
    cmp byte ptr [rax + 0x1a9], 0
    jne none

    ; pass 1: a free slot of this type goes to a member holding nothing
    mov eax, dword ptr [rbx + r14*8 + 0x264]
    cmp dword ptr [rbx + r14*8 + 0x260], eax
    jge swap
    xor esi, esi
p1_loop:
    mov rcx, qword ptr [rbx + 0x1e8]
    mov rax, qword ptr [rbx + 0x1f0]
    sub rax, rcx
    sar rax, 3
    cmp rsi, rax
    jae swap
    mov rdi, qword ptr [rcx + rsi*8]
    mov rax, qword ptr [rdi]
    mov rcx, rdi
    call qword ptr [rax + 0x180]
    test eax, eax
    jne p1_next
    jmp take
p1_next:
    inc rsi
    jmp p1_loop

    ; pass 2: swapping one weapon for another
swap:
    test r13b, r13b
    je none
    xor esi, esi
p2_loop:
    mov rcx, qword ptr [rbx + 0x1e8]
    mov rax, qword ptr [rbx + 0x1f0]
    sub rax, rcx
    sar rax, 3
    cmp rsi, rax
    jae p2_fallback
    mov rdi, qword ptr [rcx + rsi*8]
    mov rax, qword ptr [rdi]
    mov rcx, rdi
    call qword ptr [rax + 0x180]
    cmp eax, r14d
    jne p2_next

    ; a matching member: keep the first one as the stock answer
    test rbp, rbp
    jne p2_item
    mov rax, qword ptr [rdi]
    mov rcx, rdi
    call qword ptr [rax + 0xc8]
    mov rbp, rax
p2_item:
    mov rcx, qword ptr [rdi + 0x28]
    test rcx, rcx
    je take
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x160]
    cmp rax, r12
    je p2_next
    jmp take
p2_next:
    inc rsi
    jmp p2_loop

p2_fallback:
    mov rax, rbp
    jmp done

take:
    mov rax, qword ptr [rdi]
    mov rcx, rdi
    call qword ptr [rax + 0xc8]
    jmp done

none:
    xor eax, eax

done:
    add rsp, 0x20
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret
