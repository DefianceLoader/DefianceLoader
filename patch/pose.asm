; Lie down and stand up for the marked soldiers only.
;
; The panel's lie-down and stand-up buttons send net::ChangePoseCommand: a list
; of entity ids, the selected squads, and a flag. Its handler (logic.dll
; fn_31ee20) calls lie-down (fn_1054c0, at 0x31eeb9) or stand-up (fn_105620, at
; 0x31eec0) once per entity. Both take a squad (kind 0x10) or a lone soldier
; (kind 0x20) and hand that entity's own AI facet the order, the way the pickup
; order reaches one man, so a soldier can be ordered down on his own.
;
; Marks decide only while they discriminate, as in the move filter: when some
; but not all of a squad's selectable soldiers are marked, each marked soldier
; gets the order instead of the squad. Nobody marked, or everybody, and the
; squad gets it as before. A selectable soldier is one whose facet is enabled;
; a marked one is one isSelected (vt+0x58, the patched getter) answers for.
;
; That handler is the networked path. In single-player the panel's buttons
; order each selected entity directly instead, through AiUtilsImpl vt+0x70 and
; vt+0x78 (logic.dll 0x10f710 and 0x10f720), two thunks that tail-jump to the
; same stock functions. Nothing else reaches those functions, so taking both
; paths takes every player-issued lie-down and stand-up and nothing automatic.
;
; The handler's two calls are retargeted to pose_down and pose_up. Each
; thunk's jmp becomes a call to pose_down_direct or pose_up_direct followed by
; a ret written into its int3 padding. The block may sit anywhere, so every
; entry finds its stock function from a return point at a known rva.
;
; Every call, and every soldier ordered, goes into the manager trace ring with
; kind bit 60 (down) or 61 (up), so --select-probe shows what the panel sent
; and what was done with it.
;
; The prologue is the shape tools/build.py describes in its unwind record,
; which covers `pose` up to the entries; they and the recorder use no stack.

pose:                                  ; rcx the entity, rax the stock function, r10 the ring tag
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x28
    mov r15, rax                       ; the stock lie-down or stand-up
    mov r14, r10                       ; tagged for the ring
    mov rbx, rcx                       ; the entity the panel sent
    mov rdx, rcx
    call pose_record
    test rbx, rbx
    jz whole                           ; the stock function refuses nothing itself
    mov rax, qword ptr [rbx]
    mov edx, 0x10
    mov rcx, rbx
    call qword ptr [rax + 0x98]        ; a squad?
    test al, al
    jz whole

    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0xb0]        ; its facets
    test rax, rax
    jz whole
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz whole
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz whole
    mov rdx, qword ptr [rax]
    mov rcx, rax
    call qword ptr [rdx + 0x68]        ; the member vector, as select-squad.asm reads it
    mov r12, qword ptr [rax]
    mov r13, qword ptr [rax + 8]

    xor esi, esi                       ; marked
    xor edi, edi                       ; selectable
    mov rbp, r12
count:
    cmp rbp, r13
    jae counted
    mov rcx, qword ptr [rbp]
    add rbp, 8
    test rcx, rcx
    jz count
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz count
    mov rcx, qword ptr [rax + 0x50]    ; his selectable facet
    test rcx, rcx
    jz count
    cmp byte ptr [rcx + 0x18], 0
    je count                           ; not selectable: on neither side of the rule
    inc edi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz count
    inc esi
    jmp count
counted:
    test esi, esi
    jz unpin                           ; nobody marked
    cmp esi, edi
    jae unpin                          ; everybody marked

    ; the squad's own facet, which a soldier's +0x28 names: only a facet that
    ; names it is written, so the pin cannot land in anything but a soldier's
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz whole
    mov rax, qword ptr [rax + 0x50]
    test rax, rax
    jz whole
    mov qword ptr [rsp + 0x20], rax

    mov rbp, r12
order:
    cmp rbp, r13
    jae done
    mov rsi, qword ptr [rbp]
    add rbp, 8
    test rsi, rsi
    jz order
    mov rax, qword ptr [rsi]
    mov rcx, rsi
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz order
    mov rdi, qword ptr [rax + 0x50]    ; his selectable facet
    test rdi, rdi
    jz order
    mov rax, qword ptr [rdi]
    mov rcx, rdi
    call qword ptr [rax + 0x58]
    test al, al
    jz order
    mov rax, qword ptr [rsp + 0x20]
    cmp qword ptr [rdi + 0x28], rax
    jne pinned                         ; not this squad's soldier facet: no pin
    mov eax, 1                         ; pinned standing, or
    bt r14, 60
    jnc pin
    mov eax, 3                         ; pinned prone
pin:
    mov byte ptr [rdi + 0x31], al
    mov word ptr [rdi + 0x32], 0x7a5e
pinned:
    mov rdx, rsi
    call pose_record
    mov rcx, rsi
    call r15                           ; this one soldier
    jmp order

; the order is for the whole squad: every soldier goes back to its posture
unpin:
    mov rbp, r12
unpin_next:
    cmp rbp, r13
    jae whole
    mov rcx, qword ptr [rbp]
    add rbp, 8
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
    jne unpin_next                     ; only a pin this wrote is cleared
    mov word ptr [rcx + 0x32], 0
    mov byte ptr [rcx + 0x31], 0
    jmp unpin_next

whole:
    mov rcx, rbx
    call r15
done:
    add rsp, 0x28
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret

; the handler's two retargeted calls land here, and pose returns to it
pose_down:
    mov r10, qword ptr [rsp]           ; the handler's return point, 0x31eebe
    lea rax, [r10 + 0x1054c0 - 0x31eebe]
    bts r10, 60
    jmp pose
pose_up:
    mov r10, qword ptr [rsp]           ; 0x31eec5
    lea rax, [r10 + 0x105620 - 0x31eec5]
    bts r10, 61
    jmp pose

; the AiUtils thunks' calls land here; the thunk's call left rsp 16-byte
; aligned, so calling pose rather than jumping to it is what realigns it, and
; the ring gets the thunk's caller rather than the thunk
pose_down_direct:
    mov rax, qword ptr [rsp]           ; the thunk's return point, 0x10f718
    lea rax, [rax + 0x1054c0 - 0x10f718]
    mov r10, qword ptr [rsp + 8]
    bts r10, 60
    call pose
    ret
pose_up_direct:
    mov rax, qword ptr [rsp]           ; 0x10f728
    lea rax, [rax + 0x105620 - 0x10f728]
    mov r10, qword ptr [rsp + 8]
    bts r10, 61
    call pose
    ret

; rdx the subject, r14 the tagged return point; leaf, clobbers rax and r11
pose_record:
    lea r11, [rip + {cursor}]          ; the manager trace ring
    mov rax, qword ptr [r11]
    inc qword ptr [r11]
    and rax, 31
    shl rax, 4
    mov qword ptr [r11 + rax + 0x10], r14
    mov qword ptr [r11 + rax + 0x18], rdx
    ret
