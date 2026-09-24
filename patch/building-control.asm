; Native TAB has an unused +38 slot in its verified 40-byte local frame.
; The event-local anchor is backed by an engine-owned reference during a cycle.
building_tab_collect:
    mov r9d, 1
    call building_focus_candidates
    mov qword ptr [rsp + 0x38], rdx
    mov r11, 0xaaaaaaaaaaaaaab2
    jmp r11
building_tab_apply:
    mov r8, qword ptr [rsp + 0x38]
    mov r9, rbx
    call building_focus_apply
    mov r11, 0xaaaaaaaaaaaaaab3
    jmp r11

building_tab_refresh:
    xor r9d, r9d
    call building_focus_candidates
    mov r11, 0xaaaaaaaaaaaaaab6
    jmp r11

; The building panel's squad icons (UnitInfo passenger slots, handler
; game+364b20) clear the selection and select the icon's whole squad (manager
; vt+60), members outside the building too. With a building selected, select
; only that squad's enabled members inside it, through the soldier setter as
; TAB does. Anything else (no building, not a squad, nobody inside) keeps the
; stock select. Entered after the manager lookup: rax the manager, rdi the
; icon's entity; resumes at the handler's epilogue, which restores rbx/rdi.
building_icon_select:
    push rsi
    push r12
    push r13
    sub rsp, 0x28
    mov rbx, rax                       ; displaced: the manager
    xor r12d, r12d
    mov rsi, qword ptr [rbx + 0x28]    ; the manager's registry: find the
    mov r13, qword ptr [rbx + 0x30]    ; selected building before clearing
icon_find_building:
    cmp rsi, r13
    jae icon_found
    mov rcx, qword ptr [rsi]
    add rsi, 8
    mov qword ptr [rsp + 0x20], rcx
    call focus_active
    test rax, rax
    jz icon_find_building
    mov r12, rax                       ; its ActiveBuilding, if selected
    mov rcx, qword ptr [rsp + 0x20]
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz icon_not_selected
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz icon_not_selected
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jnz icon_found
icon_not_selected:
    xor r12d, r12d
    jmp icon_find_building
icon_found:
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x98]        ; displaced: clear the selection
    test r12, r12
    jz icon_stock
    mov rcx, rdi
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]
    test al, al
    jz icon_stock                      ; not a squad
    mov rsi, qword ptr [r12 + 0x1a8]
    mov r13, qword ptr [r12 + 0x1b0]
    xor r12d, r12d                     ; now the count marked
icon_next:
    cmp rsi, r13
    jae icon_marked
    mov rax, qword ptr [rsi]
    add rsi, 8
    test rax, rax
    jz icon_next
    mov rcx, qword ptr [rax + 0x10]
    call focus_member
    test rax, rax
    jz icon_next
    mov qword ptr [rsp + 0x20], rax
    mov rcx, rax
    call focus_group
    cmp rax, rdi
    jne icon_next
    mov rcx, qword ptr [rsp + 0x20]
    mov rax, qword ptr [rcx]
    mov edx, 1
    call qword ptr [rax + 0x50]
    inc r12
    jmp icon_next
icon_marked:
    test r12, r12
    jnz icon_done
icon_stock:
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    mov rdx, rdi
    call qword ptr [rax + 0x60]        ; the stock select
icon_done:
    add rsp, 0x28
    pop r13
    pop r12
    pop rsi
    mov r11, 0xaaaaaaaaaaaaaaba        ; fixup: resume game+364bb4
    jmp r11

; RCX world, RDX game context, R8 native vector, RAX world vtable.
; Preserve native RAX; return event-local building in RDX, or zero for stock TAB.
building_focus_candidates:
    push rbx
    push r12
    push r14
    push r15
    sub rsp, 0x58
    mov r12, r8
    mov dword ptr [rsp + 0x48], r9d
    mov qword ptr [rsp + 0x28], rcx
    mov qword ptr [rsp + 0x30], rdx
    mov qword ptr [rsp + 0x40], 0
    call qword ptr [rax + 0x540]
    mov qword ptr [rsp + 0x38], rax
    mov rcx, qword ptr [rsp + 0x28]
    mov rdx, qword ptr [rsp + 0x30]
    mov r8, r12
    call focus_validate
    test rax, rax
    jz candidates_new
    cmp dword ptr [rsp + 0x48], 0
    je candidates_done
    mov rbx, rax
    mov rcx, rax
    call focus_active
    jmp candidates_building
candidates_new:
    call focus_clear
    cmp dword ptr [rsp + 0x48], 0
    je candidates_done
    mov rax, qword ptr [r12]
    mov rcx, qword ptr [r12 + 8]
    sub rcx, rax
    cmp rcx, 8
    jne candidates_done
    mov rbx, qword ptr [rax]
    mov rcx, rbx
    call focus_active
    test rax, rax
    jz candidates_done
candidates_building:
    mov qword ptr [rsp + 0x40], rbx
    mov r14, qword ptr [rax + 0x1a8]
    mov r15, qword ptr [rax + 0x1b0]
    mov rax, qword ptr [r12]
    mov qword ptr [rax], rbx
    lea rcx, [rax + 8]
    mov qword ptr [r12 + 8], rcx
candidates_next:
    cmp r14, r15
    je candidates_done
    mov rax, qword ptr [r14]
    add r14, 8
    test rax, rax
    jz candidates_next
    mov rcx, qword ptr [rax + 0x10]
    call focus_member
    test rax, rax
    jz candidates_next
    mov rcx, rax
    call focus_group
    test rax, rax
    jz candidates_next
    mov rdx, rax
    mov rcx, r12
    call focus_append_unique
    jmp candidates_next
candidates_done:
    mov rdx, qword ptr [rsp + 0x40]
    mov rax, qword ptr [rsp + 0x38]
    add rsp, 0x58
    pop r15
    pop r14
    pop r12
    pop rbx
    ret

; Collection vt+540 takes a game context, but manager vt+700 takes its player.
; Match SmartCursorCmdSelect's context->vt+40() -> world->vt+700(player).
; Passing the context itself reaches Storage with the wrong object layout.
focus_manager:
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    test rcx, rcx
    jz manager_none
    test rdx, rdx
    jz manager_none
    mov rcx, rdx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x40]
    test rax, rax
    jz manager_done
    mov rdx, rax
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x700]
    jmp manager_done
manager_none:
    xor eax, eax
manager_done:
    add rsp, 0x20
    pop rbx
    ret

; Enabled individual selectable facet; never a squad or building facet.
focus_member:
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    test rcx, rcx
    jz member_none
    mov rax, qword ptr [rcx]
    mov edx, 0x20
    call qword ptr [rax + 0x98]
    test al, al
    jz member_none
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz member_none
    mov rax, qword ptr [rax + 0x50]
    test rax, rax
    jz member_none
    cmp byte ptr [rax + 0x18], 0
    jne member_done
member_none:
    xor eax, eax
member_done:
    add rsp, 0x20
    pop rbx
    ret

; Verified soldier parent-facet chain. Standalone soldiers are their own group.
focus_group:
    mov rax, qword ptr [rcx + 0x28]
    cmp rax, 0x10000
    jb group_own
    mov rdx, 0x800000000000
    cmp rax, rdx
    jae group_own
    cmp qword ptr [rax], 0
    je group_own
    mov rax, qword ptr [rax + 0x10]
    test rax, rax
    jz group_own
    mov rax, qword ptr [rax + 0x10]
    test rax, rax
    jnz group_done
group_own:
    mov rax, qword ptr [rcx + 0x10]
    test rax, rax
    jz group_done
    mov rax, qword ptr [rax + 0x10]
group_done:
    ret

; Building with a well-formed native occupant vector, otherwise null.
focus_active:
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    test rcx, rcx
    jz active_none
    mov rax, qword ptr [rcx]
    mov edx, 0x200
    call qword ptr [rax + 0x98]
    test al, al
    jz active_none
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz active_done
    mov rax, qword ptr [rax + 0x10]
    test rax, rax
    jz active_done
    mov rax, qword ptr [rax + 0x160]
    test rax, rax
    jz active_done
    mov rcx, qword ptr [rax + 0x1a8]
    mov rdx, qword ptr [rax + 0x1b0]
    cmp rdx, rcx
    jb active_none
    sub rdx, rcx
    test dl, 7
    jnz active_none
    test rcx, rcx
    jnz active_done
    test rdx, rdx
    jz active_done
active_none:
    xor eax, eax
active_done:
    add rsp, 0x20
    pop rbx
    ret

focus_append_unique:
    mov rax, qword ptr [rcx]
    mov r9, qword ptr [rcx + 8]
append_scan:
    cmp rax, r9
    je append_new
    cmp qword ptr [rax], rdx
    je append_done
    add rax, 8
    jmp append_scan
append_new:
    cmp r9, qword ptr [rcx + 0x10]
    je append_grow
    mov qword ptr [r9], rdx
    add qword ptr [rcx + 8], 8
append_done:
    ret
append_grow:
    sub rsp, 0x28
    mov qword ptr [rsp + 0x20], rdx
    lea r8, [rsp + 0x20]
    mov rdx, r9
    mov r11, 0xaaaaaaaaaaaaaab4
    call r11
    add rsp, 0x28
    ret

; RCX focus ref, RDX next group, R8 event-local building, R9 UI.
; Real selection feeds native command availability and existing subset filters.
; Plain TAB selects every occupant, then only moves focus squad by squad. With
; the squad modifier held, each press selects one squad's occupants instead:
; the squad in focus when leaving an all-occupant cycle, else the next one.
; State +20 is the squad selected alone (0: all occupants), compared only.
building_focus_apply:
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x30
    mov r12, rcx
    mov r13, rdx
    mov r14, r8
    mov r15, r9
    test r8, r8
    jz apply_focus
    mov qword ptr [rsp + 0x28], 0   ; mark every occupant
    cmp r13, r14
    je apply_change
    call tab_modifier_down
    test ax, ax
    js apply_squad
    lea rax, [rip + {scratch}]
    cmp qword ptr [rax], 0
    je apply_change                 ; a new cycle: every occupant
    cmp qword ptr [rax + 0x20], 0
    jne apply_change                ; one squad alone: widen to every occupant
    jmp apply_focus                 ; retain ALL marks; only native focus changes
apply_squad:
    lea rax, [rip + {scratch}]
    cmp qword ptr [rax], 0
    je apply_squad_next             ; a new cycle starts at the first squad
    cmp qword ptr [rax + 0x20], 0
    jne apply_squad_next
    mov rcx, qword ptr [r12]        ; every occupant: the focused squad first
    test rcx, rcx
    jz apply_squad_next
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz apply_squad_next
    cmp rcx, r14
    je apply_squad_next
    mov r13, rcx
apply_squad_next:
    mov qword ptr [rsp + 0x28], r13
apply_change:
    mov rcx, qword ptr [r15 + 0x128]
    mov rdx, qword ptr [r15 + 0x130]
    call focus_manager
    test rax, rax
    jnz apply_manager_ready
    mov r13, r14                   ; failed lookup: retain building focus
    jmp apply_focus
apply_manager_ready:
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x98]
    cmp r13, r14
    je apply_building
    mov rcx, r14
    call focus_active
    test rax, rax
    jz apply_building
    mov rsi, qword ptr [rax + 0x1a8]
    mov rdi, qword ptr [rax + 0x1b0]
    mov qword ptr [rsp + 0x20], 0
apply_next:
    cmp rsi, rdi
    je apply_selected
    mov rax, qword ptr [rsi]
    add rsi, 8
    test rax, rax
    jz apply_next
    mov rcx, qword ptr [rax + 0x10]
    call focus_member
    test rax, rax
    jz apply_next
    mov rbx, rax
    cmp qword ptr [rsp + 0x28], 0
    je apply_mark
    mov rcx, rax
    call focus_group
    cmp rax, qword ptr [rsp + 0x28]
    jne apply_next                  ; another squad's occupant
apply_mark:
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    mov edx, 1
    call qword ptr [rax + 0x50]
    inc qword ptr [rsp + 0x20]
    jmp apply_next
apply_selected:
    cmp qword ptr [rsp + 0x20], 0
    je apply_building
    lea rax, [rip + {scratch}]
    cmp qword ptr [rax], 0
    jne apply_selected_state        ; a running cycle keeps its reference
    lea rcx, [rip + {scratch}]
    mov rdx, r14
    call focus_assign_ref
    lea rax, [rip + {scratch}]
    mov rcx, qword ptr [r15 + 0x128]
    mov qword ptr [rax + 8], rcx
    mov rcx, qword ptr [r15 + 0x130]
    mov qword ptr [rax + 0x10], rcx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x40]
    lea rdx, [rip + {scratch}]
    mov qword ptr [rdx + 0x18], rax
apply_selected_state:
    lea rax, [rip + {scratch}]
    mov rcx, qword ptr [rsp + 0x28]
    mov qword ptr [rax + 0x20], rcx
    jmp apply_focus
apply_building:
    call focus_clear
    mov r13, r14
    mov rcx, r14
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz apply_focus
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz apply_focus
    mov rax, qword ptr [rcx]
    mov edx, 1
    call qword ptr [rax + 0x50]
apply_focus:
    mov rcx, r12
    mov rdx, r13
    call focus_assign_ref
    add rsp, 0x30
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret

; AX the squad modifier's GetAsyncKeyState (sign set: held), 0 when off. The
; virtual key is in state +28, which Core writes from selection's setting.
tab_modifier_down:
    lea rax, [rip + {scratch}]
    mov ecx, dword ptr [rax + 0x28]
    test ecx, ecx
    jz modifier_off
    mov rax, 0xaaaaaaaaaaaaaabb        ; export: user32!GetAsyncKeyState
    jmp rax
modifier_off:
    xor eax, eax
    ret

; Native assignment owns/refcounts the holder; its entity slot is invalidated
; by the engine when the entity dies. Never retain a raw building pointer.
focus_assign_ref:
    mov r11, 0xaaaaaaaaaaaaaab5
    jmp r11
focus_clear:
    sub rsp, 0x28
    lea rcx, [rip + {scratch}]
    xor edx, edx
    call focus_assign_ref
    lea rax, [rip + {scratch}]
    mov qword ptr [rax + 8], 0
    mov qword ptr [rax + 0x10], 0
    mov qword ptr [rax + 0x18], 0
    mov qword ptr [rax + 0x20], 0
    add rsp, 0x28
    ret

; RCX live member, RDX live ActiveBuilding. Pointer equality only on holders.
focus_inside:
    mov r8, qword ptr [rdx + 0x1a8]
    mov r9, qword ptr [rdx + 0x1b0]
inside_next:
    cmp r8, r9
    je inside_no
    mov rax, qword ptr [r8]
    add r8, 8
    test rax, rax
    jz inside_next
    cmp qword ptr [rax + 0x10], rcx
    jne inside_next
    mov eax, 1
    ret
inside_no:
    xor eax, eax
    ret

; Does the current native selection represent this group? Some engine paths
; expose a member rather than its parent, so normalize either representation.
focus_has_group:
    push rbx
    push rsi
    push rdi
    sub rsp, 0x20
    mov rbx, rdx
    mov rsi, qword ptr [rcx]
    mov rdi, qword ptr [rcx + 8]
group_scan:
    cmp rsi, rdi
    je group_absent
    mov rcx, qword ptr [rsi]
    add rsi, 8
    cmp rcx, rbx
    je group_present
    call focus_member
    test rax, rax
    jz group_scan
    mov rcx, rax
    call focus_group
    cmp rax, rbx
    jne group_scan
group_present:
    mov eax, 1
    jmp group_checked
group_absent:
    xor eax, eax
group_checked:
    add rsp, 0x20
    pop rdi
    pop rsi
    pop rbx
    ret

; RCX world, RDX context, R8 current native selection. Return live building or
; zero. Read only: invalidating a cycle never changes the player's selection.
; The cell holds one engine-managed holder and world/context/player identities.
focus_validate:
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x48
    mov qword ptr [rsp + 0x20], r8
    lea rbx, [rip + {scratch}]
    cmp qword ptr [rbx + 8], rcx
    jne validate_no
    cmp qword ptr [rbx + 0x10], rdx
    jne validate_no
    mov rax, qword ptr [rbx]
    test rax, rax
    jz validate_no
    mov rcx, rdx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x40]
    test rax, rax
    jz validate_no
    cmp qword ptr [rbx + 0x18], rax
    jne validate_no
    mov rax, qword ptr [rbx]
    mov rcx, qword ptr [rax + 0x10]
    test rcx, rcx
    jz validate_no
    mov qword ptr [rsp + 0x28], rcx
    call focus_active
    test rax, rax
    jz validate_no
    mov r14, rax
    mov rsi, qword ptr [rax + 0x1a8]
    mov rdi, qword ptr [rax + 0x1b0]
    xor r15d, r15d
validate_occupant:
    cmp rsi, rdi
    je validate_groups
    mov rax, qword ptr [rsi]
    add rsi, 8
    test rax, rax
    jz validate_occupant
    mov rcx, qword ptr [rax + 0x10]
    call focus_member
    test rax, rax
    jz validate_occupant
    mov rbx, rax
    lea rax, [rip + {scratch}]
    mov rax, qword ptr [rax + 0x20]
    test rax, rax
    jz validate_member_selected      ; every occupant must stay selected
    mov rcx, rbx
    call focus_group
    lea rcx, [rip + {scratch}]
    cmp rax, qword ptr [rcx + 0x20]
    jne validate_occupant            ; one squad alone: only its occupants
validate_member_selected:
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz validate_no
    mov rcx, rbx
    call focus_group
    mov rdx, rax
    mov rcx, qword ptr [rsp + 0x20]
    call focus_has_group
    test al, al
    jz validate_no
    inc r15
    jmp validate_occupant
validate_groups:
    test r15, r15
    jz validate_no
    mov rax, qword ptr [rsp + 0x20]
    mov r12, qword ptr [rax]
    mov r13, qword ptr [rax + 8]
validate_group:
    cmp r12, r13
    je validate_yes
    mov rbx, qword ptr [r12]
    add r12, 8
    test rbx, rbx
    jz validate_no
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]
    test al, al
    jnz validate_roster
    mov rcx, rbx
    call focus_member
    test rax, rax
    jz validate_no
    mov rcx, rbx
    mov rdx, r14
    call focus_inside
    test al, al
    jz validate_no
    jmp validate_group
validate_roster:
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz validate_no
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz validate_no
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz validate_no
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x68]
    test rax, rax
    jz validate_no
    mov rsi, qword ptr [rax]
    mov rdi, qword ptr [rax + 8]
    xor r15d, r15d
validate_member:
    cmp rsi, rdi
    jae validate_roster_done
    mov rbp, qword ptr [rsi]
    add rsi, 8
    mov rcx, rbp
    call focus_member
    test rax, rax
    jz validate_member
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz validate_member
    mov rcx, rbp
    mov rdx, r14
    call focus_inside
    test al, al
    jz validate_no
    inc r15
    jmp validate_member
validate_roster_done:
    test r15, r15
    jz validate_no
    jmp validate_group
validate_yes:
    mov rax, qword ptr [rsp + 0x28]
    jmp validate_done
validate_no:
    xor eax, eax
validate_done:
    add rsp, 0x48
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret
