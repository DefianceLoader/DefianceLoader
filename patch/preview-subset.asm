; Mark the unselected soldiers of the squad preview, and rebuild the preview
; when the squad's selection changes.
;
; fn_216b40, the in-mission SquadPreviewDataProvider, writes one descriptor per
; soldier. Its dead byte starts as 0 in the frame's temp [rbp-0x79] and is the
; one field every copy of the descriptor carries. subset_mark (0x216c94) writes
; 2 there instead for a living soldier whose selectable facet is not selected;
; logic.dll's preview builder turns 2 into a dimmed soldier when the squad also
; has a selected one (patch/preview-dim.asm).
;
; The UnitInfo panel's set-entity (fn_364440, every frame) rebuilds the preview
; only when the shown entity changes or the preview widget ([rbx+0x118]) has
; its +0x38 dirty flag set. subset_refresh (0x36449c) redoes the read of the
; shown entity and keeps, in the cell ({scratch}), the entity and a signature of
; its members' selection. When the entity is unchanged and the signature is not,
; it sets the dirty flag and goes to the preview block (0x364563), which
; rebuilds; otherwise it resumes at the stock je (0x3644b2) with the flags of
; the entity comparison.
;
; The signature is a leading 1 and then a bit per selectable member, in roster
; order: squad entity vt+0xb0 -> +0x28 (its AI) vt+{squad_roster} vt+0x68.
;
; At both sites rsp is 16-byte aligned. subset_mark keeps rcx (the soldier)
; and redoes its displaced xorps last; rax, rdx and r8 to r11 are dead there
; and at subset_refresh, which keeps rbx, rdi and r14.

subset_mark:
    sub rsp, 0x30
    mov qword ptr [rsp + 0x28], rcx
    mov byte ptr [rbp - 0x79], 0       ; displaced: alive
    call subset_facet
    test rax, rax
    jz subset_mark_done
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jnz subset_mark_done
    mov byte ptr [rbp - 0x79], 2       ; alive, not selected
subset_mark_done:
    mov rcx, qword ptr [rsp + 0x28]
    add rsp, 0x30
    xorps xmm0, xmm0                   ; displaced
    mov r11, 0xaaaaaaaaaaaaaac4
    jmp r11

subset_refresh:
    mov rax, qword ptr [r14]           ; displaced: the shown entity's reference
    test rax, rax
    jz subset_refresh_shown
    mov rax, qword ptr [rax + 0x10]
subset_refresh_shown:
    cmp rax, rdi
    je subset_refresh_same
    call subset_record                 ; the new entity's signature
    test rsp, rsp                      ; not equal: the stock rebuild
    jmp subset_refresh_resume
subset_refresh_same:
    test rdi, rdi
    jz subset_refresh_skip             ; nothing shown
    call subset_record
    test eax, eax
    jz subset_refresh_skip
    mov rax, qword ptr [rbx + 0x118]
    test rax, rax
    jz subset_refresh_skip
    mov byte ptr [rax + 0x38], 1
    mov r11, 0xaaaaaaaaaaaaaac6
    jmp r11
subset_refresh_skip:
    xor eax, eax                       ; equal: nothing to rebuild
subset_refresh_resume:
    mov r11, 0xaaaaaaaaaaaaaac5
    jmp r11

; rdi the entity. Stores it and its signature; eax 1 when either changed.
subset_record:
    push rbx
    push rsi
    push rbp
    push r12
    sub rsp, 0x28
    mov ebp, 1
    test rdi, rdi
    jz subset_record_done
    mov rcx, rdi
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]
    test al, al
    jz subset_record_done              ; not a squad
    mov rcx, rdi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz subset_record_done
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz subset_record_done
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz subset_record_done
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x68]
    test rax, rax
    jz subset_record_done
    mov rsi, qword ptr [rax]
    mov r12, qword ptr [rax + 8]
subset_record_member:
    cmp rsi, r12
    jae subset_record_done
    mov rcx, qword ptr [rsi]
    add rsi, 8
    call subset_facet
    test rax, rax
    jz subset_record_member
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    add rbp, rbp
    test al, al
    jz subset_record_member
    or rbp, 1
    jmp subset_record_member
subset_record_done:
    lea rcx, [rip + {scratch}]
    xor eax, eax
    cmp qword ptr [rcx], rdi
    jne subset_record_store
    cmp qword ptr [rcx + 8], rbp
    je subset_record_out
subset_record_store:
    mov qword ptr [rcx], rdi
    mov qword ptr [rcx + 8], rbp
    mov eax, 1
subset_record_out:
    add rsp, 0x28
    pop r12
    pop rbp
    pop rsi
    pop rbx
    ret

; rcx an entity. Its selectable facet when it is a soldier who can be
; selected, else 0.
subset_facet:
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    test rcx, rcx
    jz subset_facet_none
    mov rax, qword ptr [rcx]
    mov edx, 0x20
    call qword ptr [rax + 0x98]
    test al, al
    jz subset_facet_none
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz subset_facet_none
    mov rax, qword ptr [rax + 0x50]
    test rax, rax
    jz subset_facet_none
    cmp byte ptr [rax + 0x18], 0
    jne subset_facet_done
subset_facet_none:
    xor eax, eax
subset_facet_done:
    add rsp, 0x20
    pop rbx
    ret
