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
    call qword ptr [rax + 0x68]
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
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz capable_keep
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz capable_keep
    mov rax, qword ptr [rcx]
    mov edx, r14d
    mov r8b, 1
    call qword ptr [rax + {ai_can_attack}]
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
;
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
selected_members:
    xor r8d, r8d
filter_members:
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    push r14
    sub rsp, 0x28
    mov r14d, r8d
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
    jz left_behind_member
    mov qword ptr [rsi + rdi*8], r13
    inc rdi
next_member:
    inc rbx
    jmp pack_members
left_behind_member:
    test r14d, r14d
    jz next_member
    test r13, r13
    jz next_member
    mov rcx, r13
    mov rax, qword ptr [rcx]
    mov edx, 0x20
    call qword ptr [rax + 0x98]       ; a soldier
    test al, al
    jz next_member
    mov rcx, r13
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz next_member
    mov rcx, qword ptr [rax + 0x58]   ; posture
    test rcx, rcx
    jz next_member
    cmp dword ptr [rcx + 0x74], 3
    jne next_member                   ; not lying prone
    mov rcx, qword ptr [rax + 0x50]   ; selectable facet
    test rcx, rcx
    jz next_member
    mov byte ptr [rcx + 0x31], 3
    mov word ptr [rcx + 0x32], 0x7a5e
    jmp next_member
keep_members:
    mov rdi, r12
packed_members:
    mov rax, rdi
    add rsp, 0x28
    pop r14
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
