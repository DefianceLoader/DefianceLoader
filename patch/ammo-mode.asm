; Ammo slot use (the AmmunitionMenu toggles) for the marked soldiers only.
;
; The roster's slot vector is shared by a squad. Pins on each soldier's
; selectable facet override its disabled flag (1 off, 0 on), for slots 0..7.
; A whole-squad command clears only that slot's pins. The UI asks ammo_get,
; which reports disabled only when every recipient has that slot disabled.
; No gun pointers are cached and queries never release or consume ammunition.

ammo_set:                              ; rcx the AI facet, rdx slot, r8d the value
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x48
    mov qword ptr [rsp + 0x20], rcx    ; the AI facet, for its own slots
    mov qword ptr [rsp + 0x28], rdx    ; the slot
    mov qword ptr [rsp + 0x30], r8     ; the value
    ; trace: who called, with which facet, slot and value (kinds 0x40/0x41)
    mov rax, qword ptr [rsp + 0x88]    ; the return address, above the pushes and frame
    mov r10, 0x4000000000000000
    or rax, r10
    mov r9, qword ptr [rsp + 0x20]
    call amo_note
    mov rax, 0x4100000000000000
    mov r9, qword ptr [rsp + 0x28]
    mov r10d, dword ptr [rsp + 0x30]
    shl r10, 32
    or r9, r10
    call amo_note
    ; trace the entry facet's ammo data object (kind 0x48), to compare with
    ; the ones the writes below reach
    mov rcx, qword ptr [rsp + 0x20]
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {ammo_pool_get}]
    test rax, rax
    jz ammo_entry_noted
    mov r9, rax
    mov rax, 0x4800000000000000
    call amo_note
ammo_entry_noted:
    mov rcx, qword ptr [rsp + 0x20]    ; virtual methods may clobber rcx
    call list_members
    test eax, eax
    jz ammo_set_single
    xor esi, esi                       ; marked
    xor edi, edi                       ; selectable
    mov rbp, r12
set_count:
    cmp rbp, r13
    jae set_counted
    call next_soldier                  ; rbx his facet, r15 his AI facet
    test rbx, rbx
    jz set_count
    inc edi
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0x58]
    test al, al
    jz set_count
    inc esi
    jmp set_count
set_counted:
    mov rax, 0x4200000000000000        ; trace: marked and selectable counts
    mov r9, rsi
    mov r10, rdi
    shl r10, 32
    or r9, r10
    call amo_note
    test esi, esi
    jz ammo_set_squad                  ; nobody marked
    cmp esi, edi
    jae ammo_set_squad                 ; everybody marked
    mov rax, 0x4400000000000000        ; trace: the marked branch
    xor r9d, r9d
    call amo_note
    mov rbp, r12
set_marked:
    cmp rbp, r13
    jae ammo_set_done
    call next_soldier
    test rbx, rbx
    jz set_marked
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0x58]
    test al, al
    jz set_marked
    mov rcx, rbx                       ; the pin lives on his selectable facet
    mov rdx, qword ptr [rsp + 0x28]
    mov r8d, dword ptr [rsp + 0x30]
    call ammo_pin
    mov rcx, r15
    call ammo_unload_disabled
    mov rax, 0x4900000000000000        ; trace: a per-soldier pin
    mov r9, rbx
    call amo_note
    jmp set_marked

; the whole squad: clear every soldier's pin, then write the shared vector
ammo_set_squad:
    mov rax, 0x4300000000000000        ; trace: the squad branch
    xor r9d, r9d
    call amo_note
    mov rbp, r12
set_clear:
    cmp rbp, r13
    jae set_clear_done
    call next_soldier
    test rbx, rbx
    jz set_clear
    mov rcx, qword ptr [rsp + 0x28]
    cmp rcx, 8
    jae set_clear
    cmp byte ptr [rbx + 0x19], 0xA5
    jne set_clear
    movzx eax, byte ptr [rbx + 0x1e]
    btr eax, ecx                       ; clear only the slot being changed
    mov byte ptr [rbx + 0x1e], al
    jmp set_clear
set_clear_done:
    mov rcx, qword ptr [rsp + 0x20]
    mov rdx, qword ptr [rsp + 0x28]
    mov r8d, dword ptr [rsp + 0x30]
    call write_one
    jmp ammo_set_done

ammo_set_single:                       ; a soldier, or no member list: unchanged
    mov rax, 0x4500000000000000        ; trace: the single/no-list branch
    xor r9d, r9d
    call amo_note
    mov rcx, qword ptr [rsp + 0x20]
    mov rdx, qword ptr [rsp + 0x28]
    mov r8d, dword ptr [rsp + 0x30]
    call write_one
ammo_set_done:
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

ammo_get:                              ; rcx the AI facet, rdx slot; eax the value
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x38
    mov qword ptr [rsp + 0x20], rcx    ; the AI facet
    mov qword ptr [rsp + 0x28], rdx    ; the slot
    call list_members
    test eax, eax
    jz ammo_get_single
    xor esi, esi                       ; marked
    xor edi, edi                       ; selectable
    mov rbp, r12
get_count:
    cmp rbp, r13
    jae get_counted
    call next_soldier
    test rbx, rbx
    jz get_count
    inc edi
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0x58]
    test al, al
    jz get_count
    inc esi
    jmp get_count
get_counted:
    cmp esi, edi
    jb get_subset
    xor esi, esi                      ; all marked: consider the whole squad
get_subset:
    mov edi, esi                      ; nonzero means filter by marks
    mov rbp, r12
get_marked:
    cmp rbp, r13
    jae get_all_disabled
    call next_soldier
    test rbx, rbx
    jz get_marked
    test edi, edi
    jz get_member
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0x58]
    test al, al
    jz get_marked
get_member:
    mov rcx, rbx
    mov rdx, qword ptr [rsp + 0x28]
    call ammo_get_pin
    cmp eax, -1
    jne get_value
    mov rcx, qword ptr [rsp + 0x20]    ; all members share this data
    mov rdx, qword ptr [rsp + 0x28]
    call get_one
get_value:
    test eax, eax
    jz ammo_get_done                   ; any enabled -> checkbox enabled
    jmp get_marked
get_all_disabled:
    mov eax, 1
    jmp ammo_get_done
ammo_get_single:
    mov rcx, qword ptr [rsp + 0x20]
    mov rdx, qword ptr [rsp + 0x28]
    call get_one
ammo_get_done:
    add rsp, 0x38
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret

; The members of the squad whose AI facet is rcx: r12 and r13 bound them and
; r14 is the squad's own facet; eax is 0 when they cannot be listed. A soldier
; facet's vt+0x3b8 is 0, so this fails for anything but a squad.
list_members:
    sub rsp, 0x28
    xor eax, eax
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz listed
    mov rbx, qword ptr [rcx + 0x10]    ; the squad entity
    test rbx, rbx
    jz listed
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz listed
    mov r14, qword ptr [rax + 0x50]    ; the squad's own facet
    mov rcx, qword ptr [rax + 0x28]
    xor eax, eax
    test rcx, rcx
    jz listed
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz listed
    mov rdx, qword ptr [rax]
    mov rcx, rax
    call qword ptr [rdx + 0x68]
    mov r12, qword ptr [rax]
    mov r13, qword ptr [rax + 8]
    mov eax, 1
listed:
    add rsp, 0x28
    ret
list_end:

; rbx = the facet of the soldier at [rbp], r15 his own AI facet, rbp advanced;
; zero when he has none, is not selectable, or is not a soldier of the squad
; whose facet is r14
next_soldier:
    sub rsp, 0x28
    xor ebx, ebx
    xor r15d, r15d
    mov rcx, qword ptr [rbp]
    add rbp, 8
    test rcx, rcx
    jz next_done
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz next_done
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz next_done
    cmp byte ptr [rcx + 0x18], 0
    je next_done                       ; not selectable
    cmp qword ptr [rcx + 0x28], r14
    jne next_done                      ; not this squad's soldier
    mov rbx, rcx
    mov r15, qword ptr [rax + 0x28]    ; his own AI facet
next_done:
    add rsp, 0x28
    ret
next_end:

; rcx the AI facet, rdx slot, r8d value: writes that slot's enabled dword
; through the facet's own data, as the stock setter does, without its publish
write_one:
    push rbx
    push rbp
    sub rsp, 0x28
    movsxd rbp, edx
    mov ebx, r8d
    mov qword ptr [rsp + 0x20], rcx    ; the AI facet written
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {ammo_pool_get}]
    test rax, rax
    jz write_done
    mov qword ptr [rsp + 0x18], rax    ; its ammo data object
    mov rax, 0x4600000000000000        ; trace: which facet is written
    mov r9, qword ptr [rsp + 0x20]
    call amo_note
    mov rax, 0x4700000000000000        ; trace: and the data object it writes
    mov r9, qword ptr [rsp + 0x18]
    call amo_note
    mov rcx, qword ptr [rsp + 0x18]
    mov r9, qword ptr [rcx]
    call qword ptr [r9 + 0x48]
    test rax, rax
    jz write_done
    mov rcx, qword ptr [rax]
    mov rdx, qword ptr [rax + 8]
    imul rbp, rbp, 0x48
    sub rdx, rcx
    cmp rbp, rdx
    jae write_done                     ; negative and out-of-range indices
    mov dword ptr [rcx + rbp + 0x3c], ebx
write_done:
    add rsp, 0x28
    pop rbp
    pop rbx
    ret
write_end:

; rcx the AI facet, rdx slot; eax that slot's enabled dword, 1 when absent
get_one:
    push rbx
    sub rsp, 0x20
    mov rbx, rdx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {ammo_pool_get}]
    test rax, rax
    jz get_default
    mov r8, qword ptr [rax]
    mov rcx, rax
    call qword ptr [r8 + 0x48]
    test rax, rax
    jz get_default
    mov rdx, qword ptr [rax]
    mov rcx, qword ptr [rax + 8]
    sub rcx, rdx
    sar rcx, 3
    movabs rax, 0x8e38e38e38e38e39
    imul rcx, rax
    cmp rbx, rcx
    jae get_default
    lea rax, [rbx + rbx*8]
    mov eax, dword ptr [rdx + rax*8 + 0x3c]
    add rsp, 0x20
    pop rbx
    ret
get_default:
    mov eax, 1
    add rsp, 0x20
    pop rbx
    ret
get_end:

; record one entry in the manager trace ring, which --select-probe reads:
; rax the tagged qword, r9 the subject. Clobbers rax, r10 and r11.
amo_note:
    lea r11, [rip + {cursor}]
    mov r10, qword ptr [r11]
    inc qword ptr [r11]
    and r10, 31
    shl r10, 4
    mov qword ptr [r11 + r10 + 0x10], rax
    mov qword ptr [r11 + r10 + 0x18], r9
    ret
amo_note_end:

; The per-soldier pin. The engine keeps one ammo-slot vector per squad and
; every member's AI facet points at it, so a marked soldier's value cannot be
; isolated there. It is kept in the spare bytes of his own
; SquadUnitSelectableFacet instead: +0x19 a marker, +0x1e the set mask and
; +0x1f the value mask, one bit per slot. Factory hooks clear these on
; creation/load; the marker makes a pin valid.
ammo_pin:                              ; rcx the facet, edx the slot, r8d the value
    cmp edx, 8
    jae pin_done
    cmp byte ptr [rcx + 0x19], 0xA5
    je pin_set
    mov byte ptr [rcx + 0x1e], 0
    mov byte ptr [rcx + 0x1f], 0
    mov byte ptr [rcx + 0x19], 0xA5
pin_set:
    mov r10d, edx
    movzx eax, byte ptr [rcx + 0x1e]
    bts eax, r10d
    mov byte ptr [rcx + 0x1e], al
    movzx eax, byte ptr [rcx + 0x1f]
    test r8d, r8d
    jz pin_zero
    bts eax, r10d
    jmp pin_store
pin_zero:
    btr eax, r10d
pin_store:
    mov byte ptr [rcx + 0x1f], al
pin_done:
    ret
pin_end:

; rcx the facet, edx the slot; eax the pinned value, -1 when he has no pin
ammo_get_pin:
    mov eax, -1
    cmp edx, 8
    jae pin_get_done
    cmp byte ptr [rcx + 0x19], 0xA5
    jne pin_get_done
    movzx r10d, byte ptr [rcx + 0x1e]
    bt r10d, edx
    jnc pin_get_done
    movzx eax, byte ptr [rcx + 0x1f]
    bt eax, edx
    setc al
    movzx eax, al
pin_get_done:
    ret
pin_get_end:

; Capture the member's selectable facet while RAX is its facet collection.
; RSI becomes SHARED AMMO DATA at 115a11, not a member AI, before both reads.
; This function's +28 stack slot is unused by stock code and is outside its
; outgoing four-register shadow space. Each invocation keeps its own pointer.
ammo_reader_owner:
    mov rsi, qword ptr [rax + 0x28]
    push r10
    mov r10, qword ptr [rax + 0x50]
    mov qword ptr [rsp + 0x30], r10
    pop r10
    test rsi, rsi
    jmp 0x1158b1
reader_owner_end:

; Preserve the original live registers and branch before restoring eax.
; Five pushes put the saved member facet at [rsp+50].
ammo_reader_check1:                    ; hook the loop's actual cmp at 115a50
    push rax
    push rcx
    push rdx
    push r10
    push r11
    mov eax, dword ptr [rcx + 0x10]
    mov r10, qword ptr [rsp + 0x50]
    call ammo_effective
    test eax, eax
    pop r11
    pop r10
    pop rdx
    pop rcx
    pop rax
    jnz reader_check1_skip
    jmp 0x115a56
reader_check1_skip:
    jmp 0x115a5d
reader_check1_end:

ammo_reader_check2:                    ; hook the cmp, after stock r12b test
    push rax
    push rcx
    push rdx
    push r10
    push r11
    mov eax, dword ptr [rdx + 0x3c]
    mov edx, ebp
    mov r10, qword ptr [rsp + 0x50]
    call ammo_effective
    cmp eax, 1
    pop r11
    pop r10
    pop rdx
    pop rcx
    pop rax
    je reader_check2_skip
    jmp 0x115a93
reader_check2_skip:
    jmp 0x115c0f
reader_check2_end:

; eax shared value, edx slot, r10 member selectable captured on this frame.
; No shared owner cell, entity-layout shortcut, engine call or SIMD clobber.
ammo_effective:
    test r10, r10
    jz effective_done
    cmp edx, 8
    jae effective_done
    cmp byte ptr [r10 + 0x19], 0xA5
    jne effective_done
    movzx r11d, byte ptr [r10 + 0x1e]
    bt r11d, edx
    jnc effective_done
    movzx eax, byte ptr [r10 + 0x1f]
    bt eax, edx
    setc al
    movzx eax, al
effective_done:
    ret
effective_end:

; All gate shims reserve Win64 shadow space. The stock method may spill
; arguments there. Keep the displaced TEST flags across their epilogues.
gate_from_rbx:
    push rdi
    sub rsp, 0x20
    mov rdi, rbx
    call ammo_gate_body
    lea rsp, [rsp + 0x20]
    pop rdi
    ret
gate_from_rbx_end:
gate_from_r13:
    push rdi
    sub rsp, 0x20
    mov rdi, r13
    call ammo_gate_body
    lea rsp, [rsp + 0x20]
    pop rdi
    ret
gate_from_r13_end:
gate_from_r15:
    push rdi
    sub rsp, 0x20
    mov rdi, r15
    call ammo_gate_body
    lea rsp, [rsp + 0x20]
    pop rdi
    ret
gate_from_r15_end:

; rcx data, rdx weapon, rdi gun. Preserve the stock special-ammo restriction
; and availability arithmetic, substituting only the disabled flag. A pin
; can enable one soldier even when the shared squad flag is disabled.
ammo_gate_body:
    push rbx
    push rsi
    push rdi
    sub rsp, 0x30
    mov rbx, rcx
    mov rsi, rdx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x60]
    mov dword ptr [rsp + 0x20], eax
    mov rcx, rdi
    mov rdx, rsi
    call gun_pin
    cmp eax, -1
    je gate_stock
    test eax, eax
    jnz gate_disabled
    ; Pinned on: only bypass the shared flag, never the special-type rule.
    cmp byte ptr [rbx + 0x38], 0
    je gate_available
    mov ecx, dword ptr [rsi + 0x11c]
    cmp ecx, 0x8000
    je gate_available
    cmp ecx, 0x4000
    je gate_available
    cmp ecx, 0x20
    je gate_available
    cmp ecx, 0x100
    je gate_available
    cmp ecx, 0x80
    jne gate_disabled
gate_available:                        ; r10 is the matched record from gun_pin
    mov eax, dword ptr [r10 + 0x2c]
    sub eax, dword ptr [r10 + 0x30]
    jmp gate_answer
gate_stock:
    mov eax, dword ptr [rsp + 0x20]
    jmp gate_answer
gate_disabled:
    xor eax, eax
gate_answer:
    add rsp, 0x30
    pop rdi
    pop rsi
    pop rbx
    test eax, eax
    ret
gate_end:

; rcx gun, rdx weapon: eax pin or -1, r10 matched record. Resolve the
; live owner through its native interfaces and require a soldier (kind 0x20).
; Vehicle selectable facets have unrelated padding and must not be read as pins.
gun_pin:
    push rbx
    push rsi
    push rdi
    sub rsp, 0x20
    mov rbx, rcx
    mov rsi, rdx
    mov rdi, qword ptr [rcx + 0x18]
    test rdi, rdi
    jz gun_pin_none
    mov rdi, qword ptr [rdi + 0x10]
    test rdi, rdi
    jz gun_pin_none
    mov rcx, rdi
    mov edx, 0x20
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x98]
    test al, al
    jz gun_pin_none
    mov rcx, rdi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz gun_pin_none
    mov r8, qword ptr [rax + 0x50]
    mov rcx, rbx
    mov rdx, rsi
    mov eax, -1
    test r8, r8
    jz gun_pin_done
    cmp byte ptr [r8 + 0x19], 0xA5
    jne gun_pin_done
    mov r9, qword ptr [rcx + 0x58]
    test r9, r9
    jz gun_pin_done
    mov r9, qword ptr [r9 + 0x10]
    test r9, r9
    jz gun_pin_done
    mov r10, qword ptr [r9 + 0x20]
    mov r11, qword ptr [r9 + 0x28]
    xor ecx, ecx
gun_pin_next:
    cmp r10, r11
    jae gun_pin_done
    cmp ecx, 8
    jae gun_pin_done
    cmp qword ptr [r10], rdx
    je gun_pin_slot
    inc ecx
    add r10, 0x48
    jmp gun_pin_next
gun_pin_slot:
    movzx r9d, byte ptr [r8 + 0x1e]
    bt r9d, ecx
    jnc gun_pin_done
    movzx eax, byte ptr [r8 + 0x1f]
    bt eax, ecx
    setc al
    movzx eax, al
    test eax, eax
    jz gun_pin_done
    lea r9, [rip + {scratch}]
    inc qword ptr [r9 + 0x40]
    mov qword ptr [r9 + 0x48], r8
    mov qword ptr [r9 + 0x50], rcx
    jmp gun_pin_done
gun_pin_none:
    mov eax, -1
gun_pin_done:
    add rsp, 0x20
    pop rdi
    pop rsi
    pop rbx
    ret
gun_pin_end:

; The loaded-round queries bypass the roster gate entirely. Apply the pin
; here too, retaining the actual reservation so re-enabling is reversible.
; These entries replace complete leaf methods (Gun vt+80/88/90/d0).
gun_ready:
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    mov rdx, qword ptr [rcx + 0x50]
    call gun_pin
    cmp eax, 1
    je ready_no
    cmp byte ptr [rbx + 0xe2], 0
    je ready_no
    cmp dword ptr [rbx + 0xdc], 0
    setne al
    jmp ready_done
ready_no:
    xor eax, eax
ready_done:
    add rsp, 0x20
    pop rbx
    ret
gun_ready_end:
gun_can_fire:
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    mov rdx, qword ptr [rcx + 0x50]
    call gun_pin
    cmp eax, 1
    je can_fire_no
    cmp byte ptr [rbx + 0xe0], 0
    jne can_fire_no
    cmp dword ptr [rbx + 0xdc], 0
    setne al
    jmp can_fire_done
can_fire_no:
    xor eax, eax
can_fire_done:
    add rsp, 0x20
    pop rbx
    ret
gun_can_fire_end:
gun_empty:
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    mov rdx, qword ptr [rcx + 0x50]
    call gun_pin
    cmp eax, 1
    je empty_yes
    cmp dword ptr [rbx + 0xdc], 0
    sete al
    jmp empty_done
empty_yes:
    mov eax, 1
empty_done:
    add rsp, 0x20
    pop rbx
    ret
gun_empty_end:
gun_loaded:
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    mov rdx, qword ptr [rcx + 0x50]
    call gun_pin
    cmp eax, 1
    je loaded_no
    mov eax, dword ptr [rbx + 0xdc]
    jmp loaded_done
loaded_no:
    xor eax, eax
loaded_done:
    add rsp, 0x20
    pop rbx
    ret
gun_loaded_end:

; Toggle-time mutation only: enumerate the soldier's live gunners and guns,
; and return any disabled weapon's reservation through Gun::release. Keeping
; dc nonzero lets the native chooser retain the disabled weapon as loaded.
; Use virtual enumeration, also used by AmmunitionMenu, instead of a cache.
ammo_unload_disabled:
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    sub rsp, 0x20
    mov rbx, rcx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {gunner_count}]
    mov r12, rax
    xor esi, esi
unload_next_gunner:
    cmp rsi, r12
    jae unload_done
    mov rcx, rbx
    mov rdx, rsi
    inc rsi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {gunner_get}]
    test rax, rax
    jz unload_next_gunner
    mov rdi, rax
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xf0]
    mov r13, rax
    xor ebp, ebp
unload_next_gun:
    cmp rbp, r13
    jae unload_next_gunner
    mov rcx, rdi
    mov rdx, rbp
    inc rbp
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xf8]
    test rax, rax
    jz unload_next_gun
    mov r14, rax
    cmp dword ptr [rax + 0xdc], 0
    je unload_next_gun
    mov rcx, rax
    mov rdx, qword ptr [rax + 0x50]
    call gun_pin
    cmp eax, 1
    jne unload_next_gun
    mov rcx, r14
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xf8]
    jmp unload_next_gun
unload_done:
    add rsp, 0x20
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret
ammo_unload_disabled_end:

; Both creation paths must invalidate padding from recycled allocations.
; Replay the displaced parent initialization without changing registers/flags.
ammo_init_new:
    mov byte ptr [rdi + 0x19], 0
    mov word ptr [rdi + 0x1e], 0
    mov qword ptr [rdi + 0x28], 0
    jmp 0x25c979
ammo_init_load:
    mov byte ptr [rdi + 0x19], 0
    mov word ptr [rdi + 0x1e], 0
    mov qword ptr [rdi + 0x28], 0
    jmp 0x25cac3
