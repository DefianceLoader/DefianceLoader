; Restrict a move order to the soldiers the player marked.
;
; fn_439f20 distributes a move order across a squad: it collects the members
; into a command-local vector and then allocates formation destinations and one
; order per member. Selecting a single soldier does not narrow that, because
; the order arrives at the squad's AI facet and the distribution walks the
; squad, so a move always moves everyone.
;
; This is called from the boundary at 0x43a032, after the members are
; collected and before the empty check and any formation work, where:
;
;   [rsp+0x30]  the vector's begin      [rsp+0x38]  its end, also in rcx
;   [rsp+0x40]  its capacity, which must not change so the cleanup still frees
;               the original allocation
;
; The rule is the one the pickup chooser uses: marks decide only while they
; discriminate. Nobody marked, or everybody marked, means no preference and the
; vector is left exactly as it was, so ordinary squad movement is untouched.
; Otherwise the vector is compacted in place onto the marked members.
;
; This filters membership only. Ownership, death, pathability and AI
; eligibility are all left to the code that follows, which is why the
; compaction happens before any of it rather than at the final handoff.
;
; entity: vt+0xb0 -> its facets, +0x50 -> the selectable facet
; selectable facet: vt+0x58 -> is it selected, which for a soldier is the
;                             patched getter: enabled, his own mark, and his
;                             squad still selected
;
; The displaced instruction, mov rdi, [rsp+0x30], is performed on the way out,
; and rcx is left holding the new end for the comparison that follows.

    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    sub rsp, 0x38

    ; the call pushed a return address, and the prologue another 0x60, so the
    ; caller's [rsp+0x30] is at [rsp+0xa0] from here. Its rsi, the order, is
    ; saved at [rsp+0x50] and its r13, the squad's AI, at [rsp+0x38].
    mov rsi, qword ptr [rsp + 0xa0]
    mov rbp, qword ptr [rsp + 0xa8]
    xor r12d, r12d                     ; marked members
    xor r13d, r13d                     ; members altogether
    mov rbx, rsi

count_next:
    cmp rbx, rbp
    jae count_done
    mov rcx, qword ptr [rbx]
    add rbx, 8
    test rcx, rcx
    jz count_next
    inc r13
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz count_next
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz count_next
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz count_next
    inc r12
    jmp count_next

count_done:
    test r12, r12
    jz keep                            ; nobody marked
    cmp r12, r13
    jae keep                           ; everybody marked, so no preference

    ; A run from a prone squad: fn_439f20 clears the squad's flag on any run
    ; (0x43a273), which would stand everyone the flag governs, not only the
    ; soldiers running. So the soldiers left behind are pinned prone before
    ; it does, and the runners lose their pins. r12 = a run, r13 = and prone.
    xor r12d, r12d
    mov rcx, qword ptr [rsp + 0x50]    ; the order
    test rcx, rcx
    jz run_known
    mov rax, qword ptr [rcx]
    mov edx, 0x200
    call qword ptr [rax + 0x70]
    movzx r12d, al
run_known:
    xor r13d, r13d
    mov rax, qword ptr [rsp + 0x38]    ; the squad's AI
    test rax, rax
    jz prone_known
    movzx r13d, byte ptr [rax + 0x29e]
prone_known:
    and r13d, r12d

    mov rdi, rsi                       ; where the kept members are written
    mov rbx, rsi
pack_next:
    cmp rbx, rbp
    jae pack_done
    mov rcx, qword ptr [rbx]
    add rbx, 8
    test rcx, rcx
    jz pack_next
    mov qword ptr [rsp + 0x20], rcx    ; the entity, across the two calls
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz pack_next
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz pack_next
    mov qword ptr [rsp + 0x28], rcx    ; his facet, across the call
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    mov rcx, qword ptr [rsp + 0x28]
    test al, al
    jz left_behind
    test r12d, r12d
    jz kept                            ; a walk keeps his posture
    cmp word ptr [rcx + 0x32], 0x7a5e
    jne kept
    mov word ptr [rcx + 0x32], 0       ; a run stands him
    mov byte ptr [rcx + 0x31], 0
kept:
    mov rax, qword ptr [rsp + 0x20]
    mov qword ptr [rdi], rax
    add rdi, 8
    jmp pack_next
left_behind:
    test r13d, r13d
    jz pack_next
    cmp word ptr [rcx + 0x32], 0x7a5e
    je pack_next                       ; his own pin already holds him
    mov byte ptr [rcx + 0x31], 3       ; stay down when the squad's flag clears
    mov word ptr [rcx + 0x32], 0x7a5e
    jmp pack_next
pack_done:
    mov rax, rdi
    jmp tail

keep:
    mov rax, rbp

tail:
    ; A run stands the soldiers that move, as it stands a prone squad: their
    ; posture pins (patch/posture-gate.asm) are cleared so the stand-up is not
    ; refused. A walk keeps each soldier's own posture, pinned or not. The
    ; order is the caller's rsi, pushed by the prologue; the run flag is 0x200,
    ; as fn_439f20 itself tests it at 0x43a264. Only a pin with its marker is
    ; cleared, so nothing else is written.
    mov r12, rax                       ; the new end, across the calls
    mov rcx, qword ptr [rsp + 0x50]    ; the order
    test rcx, rcx
    jz unpinned
    mov rax, qword ptr [rcx]
    mov edx, 0x200
    call qword ptr [rax + 0x70]
    test al, al
    jz unpinned                        ; a walk
    mov rbx, rsi
unpin_next:
    cmp rbx, r12
    jae unpinned
    mov rcx, qword ptr [rbx]
    add rbx, 8
    test rcx, rcx
    jz unpin_next
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz unpin_next
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz unpin_next
    cmp word ptr [rcx + 0x32], 0x7a5e
    jne unpin_next
    mov word ptr [rcx + 0x32], 0
    mov byte ptr [rcx + 0x31], 0
    jmp unpin_next
unpinned:
    mov rax, r12
    add rsp, 0x38
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ; rsp is back at the return address, so the caller's frame has shifted by 8
    mov qword ptr [rsp + 0x40], rax    ; the vector's new end
    mov rcx, rax                       ; what the comparison below expects
    mov rdi, qword ptr [rsp + 0x38]    ; the displaced mov rdi, [rsp+0x30]
    ret
