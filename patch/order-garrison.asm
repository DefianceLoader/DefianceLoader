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

; Vehicle seats. The transport's places check (TransportHelper vt+e8,
; fn_463ca0) takes a squad array and its count and passes when the count fits
; beside the passengers outside that array. Stock callers pass the WHOLE squad,
; so a squad larger than the free seats is refused. The adapters below ask about
; one soldier instead, so a squad fills the seats left and the rest stay out.

; Narrow a soldier's own check to that soldier: rcx = helper, rdx = copied
; squad, r8 = its count, r10 = AiTransportState2. With an owner (state +10 ->
; AI -> vt+50) and a nonzero count, the owner goes over the copy's first element
; and r8 becomes 1; otherwise all three stay native. Returns rax = helper vtable.
vehicle_owner_only:
    push rbx
    push rsi
    push rdi
    sub rsp, 0x28
    mov rbx, rcx
    mov rsi, rdx
    mov rdi, r8
    test r8, r8
    jz vehicle_owner_done
    mov rcx, qword ptr [r10 + 0x10]
    test rcx, rcx
    jz vehicle_owner_done
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz vehicle_owner_done
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x50]    ; vt:logic/AiStateMachine@Leonardo|logic/AiSubstateMachine@Leonardo
    test rax, rax
    jz vehicle_owner_done
    mov qword ptr [rsi], rax
    mov edi, 1
vehicle_owner_done:
    mov rcx, rbx
    mov rdx, rsi
    mov r8, rdi
    mov rax, qword ptr [rbx]
    add rsp, 0x28
    pop rdi
    pop rsi
    pop rbx
    ret

; Entry phase (fn_fe6d0), before the soldier walks to the vehicle. A rejection
; walks only the owner away: the native fallback moves the copy's first r15
; members, so the owner goes first and r15 becomes 1. r14 = state, r15 = count.
vehicle_capacity_enter:
    mov r10, r14
    call vehicle_owner_only
    mov r15, r8
    jmp qword ptr [rax + 0xe8]    ; vt:logic/TransportHelper@Leonardo

; Boarding phase (fn_fee10), when the soldier reaches the vehicle: a pass
; admits him (vt+a8) and a failure ends his order. rbp = state.
vehicle_capacity_arrive:
    mov r10, rbp
    call vehicle_owner_only
    jmp qword ptr [rax + 0xe8]    ; vt:logic/TransportHelper@Leonardo

; Cursor (fn_c6440, may this squad board): ask whether one of its soldiers
; fits, so the order is offered while a seat is free. rax = helper vtable,
; r8 = squad size; an empty squad stays native.
vehicle_capacity_cursor:
    test r8, r8
    jz vehicle_cursor_native
    mov r8d, 1
vehicle_cursor_native:
    jmp qword ptr [rax + 0xe8]    ; vt:logic/TransportHelper@Leonardo

; Order (fn_47740) hands each transport's free seats (esi) to the ordered
; squads in turn: a squad passes the places check and takes its size (edi) from
; esi, or is skipped (+47cfc). Ask about one soldier, and let a squad larger
; than the seats left take all of them; a transport with none left refuses.
; Replaces the check call through `sub esi, edi; js`.
vehicle_capacity_command:
    test r8, r8
    jz vehicle_command_check
    mov r8d, 1
vehicle_command_check:
    sub rsp, 0x28
    call qword ptr [rax + 0xe8]    ; vt:logic/TransportHelper@Leonardo
    add rsp, 0x28
    test al, al
    jz vehicle_command_skip
    sub esi, edi
    jns vehicle_command_take
    add esi, edi
    test edi, edi
    jle vehicle_command_skip
    test esi, esi
    jle vehicle_command_skip
    xor esi, esi                     ; the squad takes the seats left
vehicle_command_take:
    ret
vehicle_command_skip:
    lea rsp, [rsp + 8]               ; discard this adapter's CALL return
    jmp 0x47cfc                      ; native skip

; Leaving (fn_ff080, phase 2, when a soldier's order changes while he boards or
; rides): the exit slot (TransportHelper vt+c0) writes the exit position and
; returns al = 1 for a passenger it lets out. The stock code then places the
; soldier at that position either way, so a soldier refused a seat, who never
; boarded, lands at the zero it was initialised to: the map origin. On a failure
; skip the placement and end the order where he stands (phase 5).
; rsi = helper, rbp = its vt+c0, rbx = state; r8/r9 already hold the outputs.
vehicle_exit_seat:
    mov rcx, rsi
    sub rsp, 0x28
    mov byte ptr [rsp + 0x20], 1     ; the stock fifth argument
    call rbp
    add rsp, 0x28
    test al, al
    jz vehicle_exit_refused
    ret
vehicle_exit_refused:
    lea rsp, [rsp + 8]               ; discard this adapter's CALL return
    jmp 0xff2cb                      ; native phase 5

; Vehicle unload to a point (fn_107600). The stock code hands the vehicle's AI
; one unload order, which moves each passenger's WHOLE squad. A vehicle is
; boarded per soldier, like a building, so order its passengers the way the
; building branch orders its occupants: collect them into that branch's member
; vector ([rbp-51], the same liveness filter) and join it at its formation
; (+1079a5), which gives each one his own move. rdi = vehicle, rax = its vtable,
; r12 = its AiFacet, whose +190 refers to the TransportHelper (passengers
; +110/+118, raw entities). No helper or no passenger: the stock call (vt+80).
vehicle_exit_point:
    mov r10, qword ptr [r12 + 0x190]
    test r10, r10
    jz vehicle_exit_native
    mov r10, qword ptr [r10 + 0x10]
    test r10, r10
    jz vehicle_exit_native
    mov r13, qword ptr [r10 + 0x110]
    mov r14, qword ptr [r10 + 0x118]
    cmp r13, r14
    jne vehicle_exit_members
vehicle_exit_native:
    sub rsp, 0x28
    call qword ptr [rax + 0x80]    ; vt:essence/EntityImpl@Essence@Galileo
    add rsp, 0x28
    ret
vehicle_exit_members:
    xorps xmm0, xmm0                 ; the building branch's two empty vectors
    movdqu xmmword ptr [rbp - 0x39], xmm0
    mov qword ptr [rbp - 0x29], rsi
    movdqu xmmword ptr [rbp - 0x51], xmm0
    mov qword ptr [rbp - 0x41], rsi
    sub rsp, 0x28
vehicle_exit_next:
    cmp r13, r14
    je vehicle_exit_formation
    mov rbx, qword ptr [r13]
    test rbx, rbx
    jz vehicle_exit_skip
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0xb0]    ; vt:essence/EntityImpl@Essence@Galileo
    mov rcx, qword ptr [rax + 0x20]
    test rcx, rcx
    jz vehicle_exit_skip
    cmp dword ptr [rcx + 0x178], 0
    je vehicle_exit_take
    cmp byte ptr [rcx + 0x150], 0
    je vehicle_exit_skip
vehicle_exit_take:
    mov qword ptr [rbp + 0x67], rbx
    mov rdx, qword ptr [rbp - 0x49]
    cmp rdx, qword ptr [rbp - 0x41]
    je vehicle_exit_grow
    mov qword ptr [rdx], rbx
    add qword ptr [rbp - 0x49], 8
    jmp vehicle_exit_skip
vehicle_exit_grow:
    lea r8, [rbp + 0x67]
    lea rcx, [rbp - 0x51]
    call 0x5a070                     ; the vector's growing insert
vehicle_exit_skip:
    add r13, 8
    jmp vehicle_exit_next
vehicle_exit_formation:
    add rsp, 0x30                    ; and this adapter's CALL return
    mov rbx, qword ptr [rbp - 0x49]
    jmp 0x1079a5

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
