; Replacement recipient chooser for "pick up the weapon on the ground".
;
; The stock chooser is fn_43e0f0(squadHolder, slotTypeId, allowSwap). Its
; swap pass returns the first squad member whose held weapon has the wanted
; slot type, so a squad with two members of one type always hands the weapon
; to the lower-indexed member and the other can never be re-armed.
;
; This version changes two things. A soldier the player has marked takes the
; weapon outright, whenever it could go to him at all. Failing that, the swap
; pass starts its scan at a rotating cursor and leaves the cursor just past
; whoever it chose, so clicking the same weapon again dispatches the next
; matching member and the player can click until the soldier they want runs.
;
; The cursor is a single dword in a zero-filled section the patch adds. It is
; written only from the order path (the one retargeted call site at
; 0x43b7d8); the three query callers keep the stock chooser, so the cursor
; advances once per pickup command on every peer rather than once per frame
; on whichever peer happens to be drawing a cursor.
;
;   rcx  SquadHolderFacet*   members at +0x1e8/+0x1f0, script info at +0x240,
;                            per-type slot counters at +0x260 (used) / +0x264 (max)
;   edx  slot type id
;   r8b  allowSwap
;
; HumanGunner member:  vt+0xc8 -> the man, vt+0x180 -> held slot type
; entity:              vt+0xb0 -> its facets, +0x50 -> the selectable facet
; selectable facet:    vt+0x58 -> is it selected
; SquadScriptInfo:     +0x1a9 -> the noPickupGun perk (perk id 5)
;
; Registers: rbx squad, rbp members begin, r12 member count, r14 slot type,
; rsi scan start, r13 steps taken, rdi the member under test, r15 the first
; marked candidate, [rsp+0x20] the rotation index and [rsp+0x28] the unmarked
; flag, both across virtual calls.

    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x38

    mov rbx, rcx
    movsxd r14, edx
    movzx r13d, r8b

    ; noPickupGun is a property of the squad type, so test it once
    mov rax, qword ptr [rbx + 0x240]
    cmp byte ptr [rax + 0x1a9], 0
    jne none

    mov rbp, qword ptr [rbx + 0x1e8]
    mov rax, qword ptr [rbx + 0x1f0]
    sub rax, rbp
    sar rax, 3
    mov r12, rax
    test r12, r12
    je none

    ; pass 0: a soldier the player marked takes it, but only when the marks say
    ; something. The mark is the patched SquadUnitSelectableFacet: clicking one
    ; soldier sets his own flag. Selecting the whole squad marks everyone the
    ; weapon could go to, and that is no preference at all, so the marks decide
    ; only while at least one candidate is left unmarked. Otherwise this falls
    ; through and the rotation below takes over.
    ;
    ; The decision needs no counts, only whether both kinds of candidate were
    ; seen: r15 is the first marked one and [rsp+0x28] records that an unmarked
    ; one exists.
    xor r15d, r15d
    mov byte ptr [rsp + 0x28], 0
    xor esi, esi
p0_loop:
    cmp rsi, r12
    jae p0_decide
    mov rdi, qword ptr [rbp + rsi*8]

    ; could the weapon go to this member at all?
    mov rax, qword ptr [rdi]
    mov rcx, rdi
    call qword ptr [rax + 0x180]
    test eax, eax
    jne p0_swap
    ; he is carrying nothing, so he needs a free slot of this type
    mov ecx, dword ptr [rbx + r14*8 + 0x264]
    cmp dword ptr [rbx + r14*8 + 0x260], ecx
    jl p0_candidate
    jmp p0_next
p0_swap:
    ; he is carrying one of these already, so this is a swap
    cmp eax, r14d
    jne p0_next
    test r13b, r13b
    je p0_next

p0_candidate:
    mov rax, qword ptr [rdi]
    mov rcx, rdi
    call qword ptr [rax + 0xc8]        ; the man, which is the entity
    test rax, rax
    je p0_unmarked
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]        ; his facets
    test rax, rax
    je p0_unmarked
    mov rcx, qword ptr [rax + 0x50]    ; the selectable facet
    test rcx, rcx
    je p0_unmarked
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]        ; is he marked?
    test al, al
    je p0_unmarked
    test r15, r15
    jne p0_next
    mov r15, rdi
    jmp p0_next
p0_unmarked:
    mov byte ptr [rsp + 0x28], 1
p0_next:
    inc rsi
    jmp p0_loop

p0_decide:
    test r15, r15
    je p0_done                         ; nobody marked
    cmp byte ptr [rsp + 0x28], 0
    je p0_done                         ; everybody marked, so no preference
    mov rdi, r15
    jmp take
p0_done:

    ; pass 1: a free slot of this type goes to a member holding nothing
    mov eax, dword ptr [rbx + r14*8 + 0x264]
    cmp dword ptr [rbx + r14*8 + 0x260], eax
    jge swap
    xor esi, esi
p1_loop:
    cmp rsi, r12
    jae swap
    mov rdi, qword ptr [rbp + rsi*8]
    mov rax, qword ptr [rdi]
    mov rcx, rdi
    call qword ptr [rax + 0x180]
    test eax, eax
    je take
    inc rsi
    jmp p1_loop

    ; pass 2: swapping one weapon for another, starting at the cursor
swap:
    test r13b, r13b
    je none
    xor r13d, r13d
    mov esi, dword ptr [rip + {cursor}]
    cmp rsi, r12
    jb p2_loop
    xor esi, esi
p2_loop:
    cmp r13, r12
    jae none
    lea rax, [rsi + r13]
    cmp rax, r12
    jb p2_index
    sub rax, r12
p2_index:
    mov qword ptr [rsp + 0x20], rax
    mov rdi, qword ptr [rbp + rax*8]
    mov rax, qword ptr [rdi]
    mov rcx, rdi
    call qword ptr [rax + 0x180]
    cmp eax, r14d
    je p2_hit
    inc r13
    jmp p2_loop

p2_hit:
    mov rax, qword ptr [rsp + 0x20]
    inc eax
    mov dword ptr [rip + {cursor}], eax

take:
    mov rax, qword ptr [rdi]
    mov rcx, rdi
    call qword ptr [rax + 0xc8]
    jmp done

none:
    xor eax, eax

done:
    add rsp, 0x38
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret
