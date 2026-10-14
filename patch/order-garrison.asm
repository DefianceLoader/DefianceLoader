; Command-local member filtering for building entry and exit. Runs AFTER
; native collection, BEFORE per-human order creation. No persistent squad
; membership or order ownership is changed.
;
; garrison: logic+43c6cd, rbp-10 = copied members, rax = count, inside
; fn_43af70. Building exit: logic+107906/+107e30; facing: +10f806/+319cd4.
; All entries are CALLs (rsp 8 mod 16).

garrison_members:
    sub rsp, 0x28
    lea rcx, [rbp - 0x10]
    mov rdx, rax
    call entering_members
    add rsp, 0x28
    mov rdi, qword ptr [r13]          ; displaced instructions, including flags
    test rdi, rdi
    ret

; Building-panel unloads/facing: all source branches checked entity flag
; 0x200. rax is the source building's facet table. The stock code projects
; selectable+38/+40 (associated SQUADS) into a command-local vector. Instead
; project ActiveBuilding+1a8/+1b0 (actual OCCUPANT handles), populated by
; fn_65410 and cleared by fn_65880. No selection marks are consulted here.
; The returned interior pointer is a read-only vector view, never an object.
building_exit_point:
    call occupant_view               ; a leaf: no frame
    mov rdi, rax
    test rdi, rdi
    ret

building_facing_direct:
building_facing_command:
building_exit_target:
    call occupant_view               ; a leaf: no frame
    mov rbx, rax
    test rbx, rbx
    ret

occupant_view:
    test rax, rax
    jz no_occupant_view
    mov rax, qword ptr [rax + 0x10]   ; BuildingTangibleFacet
    test rax, rax
    jz no_occupant_view
    mov rax, qword ptr [rax + 0x160]  ; ActiveBuilding, same as fn_ba5c0 callers
    test rax, rax
    jz no_occupant_view
    add rax, 0x170                   ; +38/+40 alias occupant begin/end
    ret
no_occupant_view:
    xor eax, eax
    ret

; entering_members also pins each member it leaves behind that is a soldier
; lying prone. The squad then stands up (fn_43d5a0: its prone flag, SquadAiFacet
; +0x29e, cleared and every member stood); a prone pin makes
; patch/posture-gate.asm skip that soldier, and the next whole-squad posture
; order or move clears it as usual. A soldier's posture target is facets +0x58
; -> +0x74 (3 prone); the pin is the selectable facet's +0x31 (3) with the
; marker 0x7a5e at +0x32, as patch/pose.asm writes it.
entering_members:
    mov r8d, 1
    jmp filter_members
%include "order-selected.asm"

; Command acceptance: a squad need not fit in its entirety. Ask the native
; predicate about one candidate at a time; the simulation checks each actual
; entrant again. Never shorten/mutate the caller's array or change selection.
building_capacity_command:
building_capacity_transfer:
    push rbx
    push rsi
    push rdi
    sub rsp, 0x20
    mov rbx, rcx
    mov rsi, rdx
    mov rdi, r8
    test rcx, rcx
    jz no_capacity
    test rdx, rdx
    jz no_capacity
capacity_candidate:
    test rdi, rdi
    jz no_capacity
    cmp qword ptr [rsi], 0
    je next_capacity_candidate
    mov rcx, rbx
    mov rdx, rsi
    mov r8d, 1
    call 0x65060
    test al, al
    jnz capacity_done
next_capacity_candidate:
    add rsi, 8
    dec rdi
    jmp capacity_candidate
no_capacity:
    xor eax, eax
capacity_done:
    add rsp, 0x20
    pop rdi
    pop rsi
    pop rbx
    ret

; The stock state checks collect the owner's WHOLE parent squad. Capacity
; belongs to the state owner instead; selection may have changed since issue.
; state+10 -> AI reference -> AI; AI virtual +50 returns the owner entity.
; Approach's native failure branch retreats the parent squad: use count zero
; on rejection to skip it. Entry's fallback walks the copied array: use one.
building_capacity_approach:
    sub rsp, 0x28
    mov r9, rdx
    mov rdx, r15
    call capacity_state_owner
    xor edi, edi
    test al, al                     ; failure must skip the native SQUAD retreat
    setne dil
    add rsp, 0x28
    ret
building_capacity_enter:
    sub rsp, 0x28
    mov r9, rdx
    mov rdx, r14
    call capacity_state_owner
    xor r12d, r12d
    test rdx, rdx
    setne r12b
    add rsp, 0x28
    ret
capacity_state_owner:
    ; rcx = building, rdx = state, r9 = copied array. Return al = fits,
    ; rdx = owner (or null); keep the local array's first element as owner.
    push rbx
    push rsi
    sub rsp, 0x28
    mov rbx, rcx
    mov rsi, r9
    test rcx, rcx
    jz no_owner_capacity
    mov rcx, qword ptr [rdx + 0x10]
    test rcx, rcx
    jz no_owner_capacity
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz no_owner_capacity
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x50]    ; vt:logic/AiStateMachine@Leonardo|logic/AiSubstateMachine@Leonardo
    test rax, rax
    jz no_owner_capacity
    mov qword ptr [rsi], rax         ; same singleton for native failure cleanup
    mov qword ptr [rsp + 0x20], rax
    lea rdx, [rsp + 0x20]
    mov r8d, 1
    mov rcx, rbx
    call 0x65060
    mov rdx, qword ptr [rsp + 0x20]
    jmp owner_capacity_done
no_owner_capacity:
    xor eax, eax
    xor edx, edx
owner_capacity_done:
    add rsp, 0x28
    pop rsi
    pop rbx
    ret

; fn_65410 normally promotes every pending order with the entrant's parent
; squad. Promote only the actual entrant's order, so outside squadmates do not
; reserve places merely because someone in their squad entered this building.
; rbx = pending list node, rsi = entrant. Native code validated its order ref.
building_reserve_occupant:
    sub rsp, 0x28
    mov rcx, qword ptr [rbx + 0x10]
    mov rcx, qword ptr [rcx + 0x10]
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x60]    ; vt:logic/AiEnterBuildingOrder@Leonardo
    cmp rax, rsi
    lea rsp, [rsp + 0x28]
    jne reserve_other_occupant
    ret                             ; native promotion at +6573b
reserve_other_occupant:
    lea rsp, [rsp + 8]               ; discard this adapter's CALL return
    jmp 0x6577f                     ; native keep-pending path

; Native free places = total - unavailable geometry - admitted reservations.
; Saturate, so a damaged building or an older over-reserved save cannot wrap
; unsigned arithmetic and appear to have virtually unlimited room.
building_capacity_subtract:
    sub rcx, rax
    jb capacity_zero
    sub rcx, r14
    jb capacity_zero
    ret
capacity_zero:
    xor ecx, ecx
    ret
