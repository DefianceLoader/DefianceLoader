; Command-local member filtering for attack, building entry and building exit.
; These wrappers run AFTER native collection, BEFORE per-human order creation.
; No persistent squad membership or order ownership is changed.
;
; attack: logic+43bab9, rbp-10 = copied members, r13 = count.
; garrison: logic+43c6cd, rbp-10 = copied members, rax = count.
; Building exit: logic+107906/+107e30; facing: +10f806/+319cd4.
; Attack/entry sites are inside fn_43af70. All entries are CALLs (rsp 8 mod 16).

attack_members:
    sub rsp, 0x28
    lea rcx, [rbp - 0x10]
    mov rdx, r13
    call selected_members
    mov r13, rax
    add rsp, 0x28
    mov rax, qword ptr [rsp + 0x58]   ; displaced mov rax, [rsp+50]
    ret

garrison_members:
    sub rsp, 0x28
    lea rcx, [rbp - 0x10]
    mov rdx, rax
    call selected_members
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
    sub rsp, 0x28
    call occupant_view
    add rsp, 0x28
    mov rdi, rax
    test rdi, rdi
    ret

building_facing_direct:
building_facing_command:
building_exit_target:
    sub rsp, 0x28
    call occupant_view
    add rsp, 0x28
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

; rcx = borrowed command-local array, rdx = count; returns retained count.
; No marks means stock squad behavior. A subset keeps marked members in order.
; Never modify the original squad vector, allocate memory, or retain pointers.
selected_members:
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    sub rsp, 0x20
    mov rsi, rcx
    mov r12, rdx
    xor ebx, ebx
    xor edi, edi
count_members:
    cmp rbx, r12
    jae counted_members
    mov rcx, qword ptr [rsi + rbx*8]
    call member_selected
    movzx eax, al
    add rdi, rax
    inc rbx
    jmp count_members
counted_members:
    test rdi, rdi
    jz keep_members
    cmp rdi, r12
    je keep_members
    xor ebx, ebx
    xor edi, edi
pack_members:
    cmp rbx, r12
    jae packed_members
    mov r13, qword ptr [rsi + rbx*8]
    mov rcx, r13
    call member_selected
    test al, al
    jz next_member
    mov qword ptr [rsi + rdi*8], r13
    inc rdi
next_member:
    inc rbx
    jmp pack_members
keep_members:
    mov rdi, r12
packed_members:
    mov rax, rdi
    add rsp, 0x20
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret

; Use the selection plugin's effective getter, including enabled/squad state.
member_selected:
    sub rsp, 0x28
    test rcx, rcx
    jz no_selection
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz no_selection
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz no_selection
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    add rsp, 0x28
    ret
no_selection:
    xor eax, eax
    add rsp, 0x28
    ret

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
    call qword ptr [rax + 0x50]
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
    call qword ptr [rax + 0x60]
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
