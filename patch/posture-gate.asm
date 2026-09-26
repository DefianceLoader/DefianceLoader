; Per-soldier posture: a pin the squad cannot override.
;
; A soldier's posture is his own (entity facets +0x58: target +0x74, 1 standing
; and 3 prone), but his squad keeps overwriting it. The squad AI holds one flag,
; SquadAiFacet +0x29e, and the soldier's states compare themselves with it and
; correct him through fn_2aff10 (lay down) or fn_2b0200 (stand up): the idle
; check in fn_ce293, state constructors, attack-move and both movement states.
; The explicit order goes through the same two (AiPoseChangeState, fn_76b8d).
; The squad's own stand-up, fn_43d5a0 (a subset sent into a building, among
; others), clears the flag and stands every member with an inlined copy of
; fn_2b0200's body instead; squad_stand_gate covers that loop.
;
; So all three are gated here. A pinned soldier refuses the change that contradicts
; his pin, wherever it comes from, and the pin is set only when the pose split
; orders him individually, which is also the only order that agrees with it.
; A whole-squad pose order clears the squad's pins first, and the move filter
; clears the pins of the soldiers that actually move, so a move hands a soldier
; back to his squad's posture.
;
; The pin lives in padding of his SquadUnitSelectableFacet (0x38 bytes: a byte
; mark at +0x30, then a dword at +0x34): +0x31 the pin, +0x32 a 16-bit marker.
; The constructor never initialises those bytes, so only the marker makes a pin
; valid; anything else, whatever the heap left there, reads as no pin.
;
; Each gate is entered by a jmp over its function's first two instructions,
; push rbx and sub rsp, imm8, with rcx the soldier's AI object, whose +0x10 is a
; holder of his entity. It redoes those two and jumps back past them; the jump
; back is a rel32 into logic.dll, which the injector re-aims once the block has
; landed. Both functions return nothing, so refusing is a bare ret.
;
; The file patch's unwind records cover each gate up to its *_resume label,
; pin_of up to pin_end, move_posture up to move_posture_end and
; squad_stand_gate up to squad_stand_end; the displaced prologues and the ret
; lie outside them.
;
; The two movement states (fn_cfb70 at 0xcffc6, fn_d0220 at 0xd0626) decide
; from the squad's flag whether a walk is a crawl: movzx ebp, byte [rcx+0x29e],
; then crawl if that is set and the order is not a run (flag 0x200). Each read
; is a call to move_posture instead, which answers with the soldier's own pin
; when he has one, so a soldier pinned prone crawls and one pinned standing
; walks upright, whatever his squad does.

stand_up_gate:                         ; fn_2b0200, rcx the soldier's AI
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    call pin_of
    mov rcx, rbx
    add rsp, 0x20
    pop rbx
    cmp al, 3
    je gate_refuse                     ; pinned prone: stay down
stand_up_resume:
    push rbx                           ; the stand-up function's own prologue, displaced
    sub rsp, {stand_up_frame}
    jmp 0x2b0206

lie_down_gate:                         ; fn_2aff10, rcx the soldier's AI
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    call pin_of
    mov rcx, rbx
    add rsp, 0x20
    pop rbx
    cmp al, 1
    je gate_refuse                     ; pinned standing: stay up
lie_down_resume:
    push rbx                           ; the lie-down function's own prologue, displaced
    sub rsp, {lie_down_frame}
    jmp 0x2aff16

gate_refuse:
    ret

; al = the pin of the soldier whose AI object is in rcx, 0 when there is none
pin_of:
    sub rsp, 0x28
    xor eax, eax
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz pin_none
    mov rcx, qword ptr [rcx + 0x10]    ; his entity, as both functions find it
    test rcx, rcx
    jz pin_none
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]        ; his facets
    test rax, rax
    jz pin_none
    mov rcx, qword ptr [rax + 0x50]    ; his selectable facet
    xor eax, eax
    test rcx, rcx
    jz pin_none
    cmp word ptr [rcx + 0x32], 0x7a5e
    jne pin_none
    movzx eax, byte ptr [rcx + 0x31]
pin_none:
    add rsp, 0x28
    ret
pin_end:

; ebp = whether the soldier moved by this state is prone. Called over a 7-byte
; movzx ebp, byte [rcx+0x29e] (two nops follow the call), rcx his squad's AI and
; rsi the movement state, whose +0x10 holds him as the state constructors find
; him. Everything but ebp is preserved, so the state carries on untouched.
move_posture:
    push rax
    push rcx
    push rdx
    push r8
    push r9
    push r10
    push r11
    sub rsp, 0x20
    movzx ebp, byte ptr [rcx + 0x29e]  ; the squad's flag, as the displaced read had it
    mov rcx, qword ptr [rsi + 0x10]
    test rcx, rcx
    jz posture_done
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz posture_done
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x50]        ; the soldier's entity
    test rax, rax
    jz posture_done
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]        ; his facets
    test rax, rax
    jz posture_done
    mov rcx, qword ptr [rax + 0x50]    ; his selectable facet
    test rcx, rcx
    jz posture_done
    cmp word ptr [rcx + 0x32], 0x7a5e
    jne posture_done                   ; unpinned: his squad's flag stands
    xor ebp, ebp
    cmp byte ptr [rcx + 0x31], 3
    sete bpl
posture_done:
    add rsp, 0x20
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdx
    pop rcx
    pop rax
    ret
move_posture_end:

; The squad's stand-up loop (fn_43d5a0 at 0x43d639) asks each member's posture,
; rbx, cmp qword [rbx+0x28], 0, and stands him unless that is set (jne skips).
; Called over that 5-byte compare, this answers with its flags: ZF clear, so
; the jne skips him, for a soldier pinned prone, else the compare's own. The
; posture's +0x10 holds its soldier as an AI object's does, so pin_of finds
; him. Every register is preserved.
squad_stand_gate:
    push rax
    push rcx
    push rdx
    push r8
    push r9
    push r10
    push r11
    sub rsp, 0x20
    mov rcx, rbx
    call pin_of
    cmp al, 3
    je stand_pinned
    cmp qword ptr [rbx + 0x28], 0      ; the displaced compare
    jmp stand_answer
stand_pinned:
    test rsp, rsp                      ; nonzero: ZF clear, so he is skipped
stand_answer:
    lea rsp, [rsp + 0x20]              ; lea and pop keep the flags
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdx
    pop rcx
    pop rax
    ret
squad_stand_end:
