; Vehicle boarding and unloading for the garrison feature: the transport places
; check at the board order's phases, the cursor and the squad load order, the
; exit slot and the unload to a point. Assembled apart from
; patch/order-garrison.asm only for room in the monolithic block (tools/payload.py
; VEHICLE_ORDER_OFFSET); the garrison unit carries both. All entries are CALLs
; (rsp 8 mod 16). The shared member filters, {selected_members} and
; {entering_members}, are patch/order-selected.asm's copy in the garrison
; routine, passed in as symbols.

; Distribution (fn_43aab0) expands a squad's transport order into per-human
; orders. At +43acf8 its copied roster is at rsp+20, count r14, squad AI rdi.
; Both values of order+28 are boarding modes, passed into each
; AiLoadToTransportOrder; vehicle leaving uses a separate dispatcher. Keep the
; marked soldiers before the per-human loop and pin the prone ones left outside
; before the squad posture reset this call displaces.
vehicle_board_members:
    sub rsp, 0x28
    lea rcx, [rsp + 0x50]            ; the caller's rsp+20, past the CALL and frame
    mov rdx, r14
    call {entering_members}
    mov r14, rax
    add rsp, 0x28
    mov byte ptr [rdi + 0x29e], 0    ; the displaced squad posture reset
    ret

; Vehicle seats. The transport's places check (TransportHelper vt+e8,
; fn_463ca0) takes a squad array and its count and passes when the count fits
; beside the passengers outside that array. Stock callers pass the WHOLE squad,
; so a squad larger than the free seats is refused. The adapters below ask about
; one soldier instead, so a squad fills the seats left and the rest stay out.

; Narrow a soldier's own check to that soldier: rcx = helper, rdx = copied
; squad, r8 = its count, r10 = AiTransportState2. With an owner (state +10 ->
; AI -> vt+50) and a nonzero count, the owner goes over the copy's first element
; and r8 becomes 1; otherwise all three stay native. A state the squad itself
; runs (owner flag 10, as the stock code tests) boards the whole squad: native.
; Returns rax = helper vtable.
vehicle_owner_only:
    push rbx
    push rsi
    push rdi
    sub rsp, 0x38
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
    mov qword ptr [rsp + 0x28], rax
    mov rcx, rax
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]    ; vt:essence/EntityImpl@Essence@Galileo a squad?
    test al, al
    jnz vehicle_owner_done
    mov rax, qword ptr [rsp + 0x28]
    mov qword ptr [rsi], rax
    mov edi, 1
vehicle_owner_done:
    mov rcx, rbx
    mov rdx, rsi
    mov r8, rdi
    mov rax, qword ptr [rbx]
    add rsp, 0x38
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

; Order (fn_47740, LoadGroupToTransportGroupAction, which scripts and the
; board order share) hands each transport's free seats (esi) to the ordered
; squads in turn (transports [rbp+6f] up to [rbp+77], squads still unplaced in
; the vector at the caller's rsp+20/+28): a squad passes the places check and
; takes its size (edi) from esi, or is skipped (+47cfc) and tried on the next
; transport. Keep exactly that. Only a squad that no transport took whole, the
; last one unplaced, at the last transport, takes the seats that are left there,
; asking about one soldier. Scripts, whose squads fit, distribute as stock.
; A squad with marked soldiers is first narrowed to them in the command-local
; copy (rdx, r8 = its count), so the check and edi count only that subset.
; Replaces the check call through `sub esi, edi; js`.
vehicle_capacity_command:
    sub rsp, 0x38
    mov qword ptr [rsp + 0x28], rcx
    mov qword ptr [rsp + 0x30], rdx
    mov rcx, rdx
    mov rdx, r8
    call {selected_members}
    mov edi, eax
    mov r8, rax
    mov rcx, qword ptr [rsp + 0x28]
    mov rdx, qword ptr [rsp + 0x30]
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xe8]    ; vt:logic/TransportHelper@Leonardo the stock check
    test al, al
    jz vehicle_command_partial
    mov eax, esi
    sub eax, edi
    js vehicle_command_partial
    mov esi, eax
    add rsp, 0x38
    ret
vehicle_command_partial:
    test edi, edi
    jle vehicle_command_skip
    test esi, esi
    jle vehicle_command_skip
    mov rax, qword ptr [rsp + 0x68]  ; the caller's squads end (its rsp+28)
    sub rax, qword ptr [rsp + 0x60]
    cmp rax, 8
    jne vehicle_command_skip         ; another squad may still fit whole
    mov rax, qword ptr [rbp + 0x6f]
    add rax, 8
    cmp rax, qword ptr [rbp + 0x77]
    jne vehicle_command_skip         ; a later transport may fit it whole
    mov rcx, qword ptr [rsp + 0x28]
    mov rdx, qword ptr [rsp + 0x30]
    mov r8d, 1
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xe8]    ; vt:logic/TransportHelper@Leonardo
    test al, al
    jz vehicle_command_skip
    xor esi, esi                     ; the squad takes the seats left
    add rsp, 0x38
    ret
vehicle_command_skip:
    add rsp, 0x40                    ; the frame and this adapter's CALL return
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
; An aircraft (entity flag 2000) keeps the stock order too: only its unload
; state (AiUnloadTransportState) hovers, drops the ropes and lets helirope and
; helijump squads out; per-passenger moves would leave them aboard.
vehicle_exit_point:
    sub rsp, 0x28                    ; one frame for every path
    mov edx, 0x2000
    call qword ptr [rax + 0x98]    ; vt:essence/EntityImpl@Essence@Galileo an aircraft? (rcx = rdi already)
    test al, al
    jnz vehicle_exit_native
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
    mov rcx, rdi                     ; the stock call's receiver and vtable
    mov rax, qword ptr [rdi]
    call qword ptr [rax + 0x80]    ; vt:essence/EntityImpl@Essence@Galileo
    add rsp, 0x28
    ret
vehicle_exit_members:
    xorps xmm0, xmm0                 ; the building branch's two empty vectors
    movdqu xmmword ptr [rbp - 0x39], xmm0
    mov qword ptr [rbp - 0x29], rsi
    movdqu xmmword ptr [rbp - 0x51], xmm0
    mov qword ptr [rbp - 0x41], rsi
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

; The squad's transport as game.dll asks for it (AiUtils vt+390, fn_111560).
; Its command menu offers a squad with a transport only the unload order, and
; nothing while that transport flies, so a squad split between the ground and a
; vehicle could not be ordered at all. Answer null while any member is on foot
; (selectable facet enabled; a passenger's is disabled), so the menu offers what
; it offers any squad on the ground. logic.dll asks the squad AI directly, so
; its own uses of the transport stay native. Replaces the tail jump to the
; squad AI's vt+squad_transport at +111598; the site's tail then returns.
; rcx = squad AI.
vehicle_transport_query:
    push rsi
    push rdi
    push rbp
    sub rsp, 0x20
    mov rbp, rcx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_transport}]    ; vt:logic/SquadAiFacet@Leonardo
    mov rdi, rax
    test rax, rax
    jz vehicle_transport_done
    mov rcx, rbp
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]    ; vt:logic/SquadAiFacet@Leonardo
    test rax, rax
    jz vehicle_transport_done
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x68]    ; vt:logic/SquadHolderFacet@Leonardo
    test rax, rax
    jz vehicle_transport_done
    mov rsi, qword ptr [rax]
    mov rbp, qword ptr [rax + 8]
vehicle_transport_member:
    cmp rsi, rbp
    jae vehicle_transport_done
    mov rcx, qword ptr [rsi]
    add rsi, 8
    test rcx, rcx
    jz vehicle_transport_member
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]    ; vt:essence/EntityImpl@Essence@Galileo
    test rax, rax
    jz vehicle_transport_member
    mov rax, qword ptr [rax + 0x50]
    test rax, rax
    jz vehicle_transport_member
    cmp byte ptr [rax + 0x18], 0
    je vehicle_transport_member
    xor edi, edi                     ; a member on foot
vehicle_transport_done:
    mov rax, rdi
    add rsp, 0x20
    pop rbp
    pop rdi
    pop rsi
    ret
