; Record every call the selection manager receives, in the order it gets them.
;
; Diagnostic only: it changes no behaviour and is meant to come out again.
;
; The first version traced SelectableMgrFacet::select alone, which answered
; "what path drove this selection" but cannot explain a toggle: Ctrl+Shift on
; one soldier runs toggle (vt+0x70), which asks the soldier's facet whether it
; is selected and then either deselects him (vt+0x88) or adds him back through
; select. Whether a removal failed because the command never arrived, because
; toggle took the add branch, or because something re-selected him afterwards
; is only visible with all four entry points on one timeline. So each of them
; is detoured here and they share one ring.
;
;   kind bit  entry      method              what is recorded
;   56        fn_418cb0  select   vt+0x60    caller, entity
;   57        fn_418f40  toggle   vt+0x70    caller, entity
;   58        fn_419350  deselect vt+0x88    caller, entity
;   59        fn_4194f0  clear    vt+0x98    caller, the manager
;
; Ring at the trace area: +0x00 the next index, which is also a running count;
; 32 entries of sixteen bytes from +0x10. The first qword is the caller's
; return address with the kind bit set, which is safe because a user-space
; address never reaches bit 47. Every hook is a jmp over whole instructions, so
; on entry rsp still points at the return address the caller pushed.
;
; Each stub records first and then performs its displaced instructions just
; before jumping back, except select, whose displaced prologue builds a frame
; the recorder then has to look past. Only rax, rcx and r11 are used, all saved
; around the recording; r11 carries the jump home, which none of the four
; entries reads.

select_trace:
    push rbx                           ; displaced: 40 53
    sub rsp, 0xc0                      ; displaced: 48 81 ec c0 00 00 00
    push rax
    push rcx
    push r11
    lea r11, [rip + {cursor}]
    mov rax, qword ptr [r11]
    inc qword ptr [r11]
    and rax, 31
    shl rax, 4
    lea rcx, [r11 + rax + 0x10]
    mov rax, qword ptr [rsp + 0xe0]    ; three pushes, the 0xc0 frame and rbx
    bts rax, 56
    mov qword ptr [rcx], rax
    mov qword ptr [rcx + 8], rdx
    pop r11
    pop rcx
    pop rax
    mov r11, 0xbbbbbbbbbbbbbbb1        ; fixup: fn_418cb0 + 9
    jmp r11

toggle_trace:
    push rax
    push rcx
    push r11
    lea r11, [rip + {cursor}]
    mov rax, qword ptr [r11]
    inc qword ptr [r11]
    and rax, 31
    shl rax, 4
    lea rcx, [r11 + rax + 0x10]
    mov rax, qword ptr [rsp + 0x18]    ; the return address, above three pushes
    bts rax, 57
    mov qword ptr [rcx], rax
    mov qword ptr [rcx + 8], rdx
    pop r11
    pop rcx
    pop rax
    mov qword ptr [rsp + 8], rbx       ; displaced: 48 89 5c 24 08
    mov r11, 0xbbbbbbbbbbbbbbb2        ; fixup: fn_418f40 + 5
    jmp r11

deselect_trace:
    push rax
    push rcx
    push r11
    lea r11, [rip + {cursor}]
    mov rax, qword ptr [r11]
    inc qword ptr [r11]
    and rax, 31
    shl rax, 4
    lea rcx, [r11 + rax + 0x10]
    mov rax, qword ptr [rsp + 0x18]
    bts rax, 58
    mov qword ptr [rcx], rax
    mov qword ptr [rcx + 8], rdx
    pop r11
    pop rcx
    pop rax
    sub rsp, 0x28                      ; displaced: 48 83 ec 28
    mov rax, qword ptr [rdx]           ; displaced: 48 8b 02
    mov r11, 0xbbbbbbbbbbbbbbb3        ; fixup: fn_419350 + 7
    jmp r11

clear_trace:
    push rax
    push rcx
    push r11
    lea r11, [rip + {cursor}]
    mov rax, qword ptr [r11]
    inc qword ptr [r11]
    and rax, 31
    shl rax, 4
    lea rcx, [r11 + rax + 0x10]
    mov rax, qword ptr [rsp + 0x18]
    bts rax, 59
    mov qword ptr [rcx], rax
    mov rax, qword ptr [rsp + 8]       ; the manager, as it arrived in rcx
    mov qword ptr [rcx + 8], rax
    pop r11
    pop rcx
    pop rax
    mov qword ptr [rsp + 8], rbx       ; displaced: 48 89 5c 24 08
    mov r11, 0xbbbbbbbbbbbbbbb4        ; fixup: fn_4194f0 + 5
    jmp r11
