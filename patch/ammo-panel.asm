; The ammunition panel, its slot steps and the order buttons' attack test, for
; the ammunition feature. A game.dll patch like patch/icon-squad.asm; it shares no code with
; that file's selection entries.

; AmmunitionMenu::fill: r12 selected entity, rbp slot, rsi shared record,
; rdi menu. Aggregate only users of this weapon and pass fillSlot a private
; record with a valid binary flag; after the stock draw, show enabled/selected
; when mixed and set the reload bar to the users' ready share. fillSlot sets
; the quantity colour itself, so a mixed slot keeps the native colour.
ammo_panel:
    sub rsp, 0x90
    mov rcx, r12
    mov rdx, rsi
    mov r8, rbp
    call ammo_ui_state
    movss dword ptr [rsp + 0x6c], xmm0
    mov dword ptr [rsp + 0x68], eax
    mov dword ptr [rsp + 0x70], r8d
    mov dword ptr [rsp + 0x74], r9d
    test al, al
    jz ammo_panel_hide
    movdqu xmm0, xmmword ptr [rsi]
    movdqu xmm1, xmmword ptr [rsi + 0x10]
    movdqu xmm2, xmmword ptr [rsi + 0x20]
    movdqu xmm3, xmmword ptr [rsi + 0x30]
    mov rax, qword ptr [rsi + 0x40]
    movdqu xmmword ptr [rsp + 0x20], xmm0
    movdqu xmmword ptr [rsp + 0x30], xmm1
    movdqu xmmword ptr [rsp + 0x40], xmm2
    movdqu xmmword ptr [rsp + 0x50], xmm3
    mov qword ptr [rsp + 0x60], rax
    mov eax, dword ptr [rsp + 0x74]
    mov dword ptr [rsp + 0x54], eax   ; private record troop count
    xor eax, eax
    cmp dword ptr [rsp + 0x68], 2     ; only usable recipients vote: all off
    sete al
    mov dword ptr [rsp + 0x5c], eax
    lea r8, [rsp + 0x20]
    mov rdx, rbp
    mov rcx, rdi
    mov r11, 0xaaaaaaaaaaaaaaaf
    call r11
    ; The reload bar (slot+0x48) shows the ready share: the stock draw fills it
    ; for any squad or multi-gun vehicle, however many users are disabled or
    ; reloading. A negative share leaves the native bar alone.
    movss xmm0, dword ptr [rsp + 0x6c]
    xorps xmm1, xmm1
    comiss xmm0, xmm1
    jb ammo_panel_slot
    mov eax, 0x3f800000
    movd xmm1, eax
    minss xmm0, xmm1
    imul rax, rbp, 0xb8
    mov rcx, qword ptr [rdi + rax + 0x1c8]
    test rcx, rcx
    jz ammo_panel_slot
    ucomiss xmm0, dword ptr [rcx + 0x1a8]
    je ammo_panel_reload_shown
    movss dword ptr [rcx + 0x1a8], xmm0
    mov r11, 0xaaaaaaaaaaaaaab7        ; the progress bar's refresh
    call r11
ammo_panel_reload_shown:
    imul rax, rbp, 0xb8
    mov rcx, qword ptr [rdi + rax + 0x1c8]
    cmp byte ptr [rcx + 0x5b], 0
    jne ammo_panel_slot
    mov rax, qword ptr [rcx]
    mov dl, 1
    call qword ptr [rax + 0x48]
ammo_panel_slot:
    imul rax, rbp, 0xb8
    lea r10, [rdi + rax + 0x180]
    mov rcx, qword ptr [r10 + 0x40]
    mov qword ptr [rsp + 0x78], rcx
    cmp dword ptr [rsp + 0x68], 3
    jne ammo_panel_count
    mov dword ptr [r10 + 8], 1        ; next click enables all selected users
ammo_panel_count:
    cmp qword ptr [rsp + 0x78], 0
    je ammo_panel_done
    ; Build selected count, or enabled/selected when mixed. A private long
    ; std::string points at the stack buffer; the native setter only copies.
    lea r10, [rsp + 0x40]
    mov eax, dword ptr [rsp + 0x74]
    cmp dword ptr [rsp + 0x68], 3
    jne ammo_panel_total
    mov eax, dword ptr [rsp + 0x70]
    call ammo_ui_number
    mov byte ptr [r10], 0x2f
    inc r10
    mov eax, dword ptr [rsp + 0x74]
ammo_panel_total:
    call ammo_ui_number
    mov byte ptr [r10], 0
    lea rax, [rsp + 0x40]
    mov qword ptr [rsp + 0x20], rax
    sub r10, rax
    mov qword ptr [rsp + 0x30], r10
    mov qword ptr [rsp + 0x38], 31
    mov rcx, qword ptr [rsp + 0x78]
    lea rdx, [rsp + 0x20]
    mov r11, 0xaaaaaaaaaaaaaab1
    call r11
    jmp ammo_panel_done
ammo_panel_hide:
    mov rcx, rdi
    mov rdx, rbp
    mov r11, 0xaaaaaaaaaaaaaab0
    call r11                          ; native hideSlot, retaining roster indices
ammo_panel_done:
    add rsp, 0x90
    mov r11, 0xaaaaaaaaaaaaaaae
    jmp r11

; SmartCursorCmdAttack's execute (game+327410) orders every recipient whose AI
; passes vt+368. The cursor offers attack when any selected unit can attack
; the target with enabled ammunition (ai_can_attack(kind, 1), game+355d90),
; but chose the recipients with ai_can_attack(kind, 0), or not at all
; (game+356840): a unit whose every weapon is disabled still took the order
; and fired the round already chambered, then stopped. Require (kind, 1) of
; each recipient too. rcx is the recipient's AI, rsi the command; a point
; target (no entity at +130) keeps the native test. The kind is read as the
; cursor reads it: target entity -> facets (vt+b0) +18 -> vt+68.
attack_recipient:
    sub rsp, 0x30
    mov qword ptr [rsp + 0x20], rcx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {ai_attack_order}] ; displaced: the stock recipient test
    test al, al
    jz attack_recipient_done
    mov rcx, qword ptr [rsi + 0x130]
    test rcx, rcx
    jz attack_recipient_yes
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz attack_recipient_yes
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz attack_recipient_yes
    mov rcx, qword ptr [rax + 0x18]
    test rcx, rcx
    jz attack_recipient_yes
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x68]
    mov edx, eax
    mov rcx, qword ptr [rsp + 0x20]
    mov rax, qword ptr [rcx]
    mov r8b, 1
    call qword ptr [rax + {ai_can_attack}]
    jmp attack_recipient_done
attack_recipient_yes:
    mov al, 1
attack_recipient_done:
    add rsp, 0x30
    mov r11, 0xaaaaaaaaaaaaaab8        ; fixup: resume game+3274db, test al, al
    jmp r11

; GameMenu's order buttons (game+23ec60) loop over the selection. Each unit
; sets the attack button's bit (edi bit 1) again, then clears it when it
; cannot attack, so the button followed whichever unit came last. Keep it
; when any selected unit passes that stock test: ammunition to fire,
; ai_can_attack(every kind, 0) and ai_attack_ready. The order itself goes
; only to units that can (attack_recipient). The loop keeps the selection's
; bounds at rbp-49 and rbp-41; this replaces the block from its first
; instruction to game+23f7df, where nothing outside it jumps in.
attack_button:
    push rbx
    push rsi
    push rdi
    sub rsp, 0x28
    mov rbx, qword ptr [rbp - 0x49]
    mov rsi, qword ptr [rbp - 0x41]
attack_button_next:
    cmp rbx, rsi
    jae attack_button_none
    mov rcx, qword ptr [rbx]
    add rbx, 8
    test rcx, rcx
    jz attack_button_next
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz attack_button_next
    mov rdi, qword ptr [rax + 0x28]
    test rdi, rdi
    jz attack_button_next
    mov rcx, rdi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {ammo_pool_get}]
    test rax, rax
    jz attack_button_next
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x48]
    mov rcx, qword ptr [rax + 8]
    cmp qword ptr [rax], rcx
    je attack_button_next
    mov rcx, rdi
    mov rax, qword ptr [rcx]
    xor r8d, r8d
    mov edx, 0xffffe7ff
    call qword ptr [rax + {ai_can_attack}]
    test al, al
    jz attack_button_next
    mov rcx, rdi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {ai_attack_ready}]
    test al, al
    jz attack_button_next
    jmp attack_button_done
attack_button_none:
    and dword ptr [rsp + 0x28], 0xfffffffd ; the caller's edi, saved above
attack_button_done:
    add rsp, 0x28
    pop rdi
    pop rsi
    pop rbx
    mov r11, 0xaaaaaaaaaaaaaab9        ; fixup: resume game+23f7df
    jmp r11

; The GUI base dispatch (game+2c2da0) sends WM_MOUSEWHEEL to vt+d8, both no-op
; on the AmmunitionMenu, WM_LBUTTONUP to vt+78 (its click, the toggle) and
; WM_RBUTTONUP to vt+88. Over one of its cards they step that slot by one
; soldier (ammo_step). These replace the branches, `mov rax, [rcx]; jmp
; [rax+slot]`; rcx is any GUI object, told apart as the menu by its mouse-move
; handler (vt+90), rdx the source widget, r9 the event: +0 the message, +8
; wparam (+a the wheel delta), +10 lparam. A click steps on the button's
; release: a quick second press arrives as a double-click, which the dispatch
; drops for the right button, but every release arrives. The step cell holds
; the modifier's virtual key (+0, 0 for none), whether clicks step (+4), the
; wheel's unspent delta (+8) and the camera wheel's last message (+10 the axis,
; +18 wparam, +20 lparam, +28 the notches it added; ammo_wheel_axis).
ammo_step_wheel:
    mov rax, qword ptr [rcx]
    mov r11, 0xaaaaaaaaaaaaaabc        ; AmmunitionMenu's mouse-move handler
    cmp qword ptr [rax + 0x90], r11
    jne ammo_step_wheel_native
    push rbx
    push rsi
    push rdi
    sub rsp, 0x30
    mov rbx, rcx
    mov rsi, rdx
    mov qword ptr [rsp + 0x20], r9
    movsx edi, word ptr [r9 + 0xa]
    call ammo_step_modifier
    test al, al
    jz ammo_step_wheel_done
    mov rcx, qword ptr [rsp + 0x20]
    call ammo_step_unzoom
    ; High-resolution wheels send less than a notch (120) per event: step once
    ; per whole notch, keeping the rest; a reversal drops the rest.
    lea r10, [rip + {scratch}]
    mov eax, dword ptr [r10 + 8]
    mov ecx, eax
    xor ecx, edi
    jns ammo_step_wheel_same
    xor eax, eax
ammo_step_wheel_same:
    add eax, edi
    cdq
    mov ecx, 120
    idiv ecx
    mov dword ptr [r10 + 8], edx
    test eax, eax
    jz ammo_step_wheel_done
    mov r8d, 1                          ; up: one more enabled
    jg ammo_step_wheel_step
    mov r8d, -1
ammo_step_wheel_step:
    mov rcx, rbx
    mov rdx, rsi
    call ammo_step
ammo_step_wheel_done:
    add rsp, 0x30
    pop rdi
    pop rsi
    pop rbx
    ret
ammo_step_wheel_native:
    jmp qword ptr [rax + 0xd8]

; The camera's zoom reads a wheel axis (game+2dc4a0, kind 2) that the window
; procedure feeds each message before the GUI's dispatch sees it. rcx is the
; wheel event the menu takes: take back what the axis added for that message.
; Volatile registers only.
ammo_step_unzoom:
    lea r10, [rip + {scratch}]
    mov rax, qword ptr [r10 + 0x10]
    test rax, rax
    jz ammo_step_unzoom_done
    mov rdx, qword ptr [rcx + 8]
    cmp rdx, qword ptr [r10 + 0x18]
    jne ammo_step_unzoom_clear
    mov rdx, qword ptr [rcx + 0x10]
    cmp rdx, qword ptr [r10 + 0x20]
    jne ammo_step_unzoom_clear
    mov edx, dword ptr [r10 + 0x28]
    sub dword ptr [rax + 0x20], edx
ammo_step_unzoom_clear:
    mov qword ptr [r10 + 0x10], 0
ammo_step_unzoom_done:
    ret

; The wheel axis's WM_MOUSEWHEEL case, from its enabled test to its return:
; r10 the axis (+18 enabled, +20 the notches not yet read), r9 wparam, the
; lparam above the return address. Add the whole notches as it does, and note
; what was added to which axis for which message, for ammo_step_unzoom.
ammo_wheel_axis:
    lea r11, [rip + {scratch}]
    mov qword ptr [r11 + 0x10], r10
    mov qword ptr [r11 + 0x18], r9
    mov rax, qword ptr [rsp + 0x28]
    mov qword ptr [r11 + 0x20], rax
    mov dword ptr [r11 + 0x28], 0
    cmp byte ptr [r10 + 0x18], 0
    je ammo_wheel_axis_done
    mov rax, r9
    shr rax, 16
    movsx eax, ax
    cdq
    mov ecx, 120
    idiv ecx                            ; toward zero, as its own division
    add dword ptr [r10 + 0x20], eax
    mov dword ptr [r11 + 0x28], eax
ammo_wheel_axis_done:
    ret

; With clicks on and the modifier held, a left click enables one more soldier
; and a right click disables one. Without a modifier a right click alone steps
; down and a left click keeps its toggle. Otherwise each release goes to its
; own slot.
ammo_step_left:
    mov r10d, 1
    jmp ammo_step_click
ammo_step_right:
    mov r10d, -1
ammo_step_click:
    mov rax, qword ptr [rcx]
    mov r11, 0xaaaaaaaaaaaaaabf        ; AmmunitionMenu's mouse-move handler
    cmp qword ptr [rax + 0x90], r11
    jne ammo_step_click_native
    lea r11, [rip + {scratch}]
    cmp dword ptr [r11 + 4], 0
    je ammo_step_click_native
    cmp dword ptr [r11], 0
    jne ammo_step_click_held
    test r10d, r10d
    jg ammo_step_click_native
ammo_step_click_held:
    push rbx
    push rsi
    push rdi
    sub rsp, 0x30
    mov rbx, rcx
    mov rsi, rdx
    mov edi, r10d
    mov qword ptr [rsp + 0x20], r8
    mov qword ptr [rsp + 0x28], r9
    call ammo_step_modifier
    test al, al
    jz ammo_step_click_release
    mov rcx, rbx
    mov rdx, rsi
    mov r8d, edi
    call ammo_step
    test al, al
    jnz ammo_step_click_done
    ; No step: refresh anyway, as the click would, so a pressed card is drawn
    ; released.
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    xorps xmm1, xmm1
    call qword ptr [rax + 0x38]
ammo_step_click_done:
    add rsp, 0x30
    pop rdi
    pop rsi
    pop rbx
    ret
ammo_step_click_release:
    mov rcx, rbx
    mov rdx, rsi
    mov r10d, edi
    mov r8, qword ptr [rsp + 0x20]
    mov r9, qword ptr [rsp + 0x28]
    add rsp, 0x30
    pop rdi
    pop rsi
    pop rbx
    mov rax, qword ptr [rcx]
ammo_step_click_native:
    test r10d, r10d
    jl ammo_step_click_native_right
    jmp qword ptr [rax + 0x78]
ammo_step_click_native_right:
    jmp qword ptr [rax + 0x88]

; al 1 when the step modifier is held or there is none. Volatile registers only.
ammo_step_modifier:
    sub rsp, 0x28
    lea r10, [rip + {scratch}]
    mov ecx, dword ptr [r10]
    mov al, 1
    test ecx, ecx
    jz ammo_step_modifier_done
    mov r11, 0xaaaaaaaaaaaaaabe        ; GetAsyncKeyState
    call r11
    test ax, 0x8000
    setnz al
ammo_step_modifier_done:
    add rsp, 0x28
    ret

; rcx menu, rdx source widget, r8d +1 or -1. Enable the first disabled user of
; the card's slot, or disable the last enabled one, in roster order, through
; the per-soldier pin (patch/ammo-mode.asm), so slots 0..7 only. Users are the
; ones the card counts (ammo_ui_state): live members, the marked ones of a
; partial selection, with a gun that takes this ammunition. One squad keeps
; this local-slot path; a combined menu dispatches to its callback so each
; source resolves the card's own ammo record. A vehicle has no per-soldier
; pins. al 1 when it handled the step and refreshed the menu.
ammo_step:
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x68
    mov qword ptr [rsp + 0x20], rcx    ; the menu
    mov dword ptr [rsp + 0x28], r8d    ; the direction
    mov qword ptr [rsp + 0x60], rdx   ; the source widget
    ; The card array ends where the press handler's search does (vt+70, a leaf
    ; that starts `lea rax, [rcx + end]`; expanded-ammo-menu moves the end).
    mov rax, qword ptr [rcx]
    mov r10, qword ptr [rax + 0x70]
    cmp word ptr [r10], 0x8d48
    jne step_done
    cmp byte ptr [r10 + 2], 0x81
    jne step_done
    movsxd r11, dword ptr [r10 + 3]
    add r11, rcx
    lea rbx, [rcx + 0x180]
    xor esi, esi
step_card:
    cmp rbx, r11
    jae step_done
    cmp qword ptr [rbx + 0x18], rdx
    je step_card_found
    cmp qword ptr [rbx + 0x20], rdx
    je step_card_found
    cmp qword ptr [rbx + 0x28], rdx
    je step_card_found
    cmp qword ptr [rbx + 0x30], rdx
    je step_card_found
    cmp qword ptr [rbx + 0x38], rdx
    je step_card_found
    add rbx, 0xb8
    inc esi
    jmp step_card
step_card_found:
    mov dword ptr [rsp + 0x2c], esi    ; the slot
    mov rcx, qword ptr [rsp + 0x20]
    mov r11, 0xaaaaaaaaaaaaaabd        ; the menu's entity (as its click reads it)
    call r11
    test rax, rax
    jz step_done
    mov qword ptr [rsp + 0x30], rax
    mov rcx, qword ptr [rsp + 0x20]
    mov rdx, rax
    call ammo_step_alone
    test al, al
    jnz step_single_squad
    ; A combined menu card uses each source squad's local slot index. The
    ; callback resolves the displayed card by weapon identity and owns pins.
    mov rax, qword ptr [rip + {scratch} + 0x30]
    test rax, rax
    jz step_done
    mov rcx, qword ptr [rsp + 0x20]
    mov rdx, qword ptr [rsp + 0x60]
    mov r8d, dword ptr [rsp + 0x28]
    call rax
    test eax, eax
    jz step_done
    jmp step_exit
step_single_squad:
    cmp esi, 8
    jae step_done                      ; pins cover local slots 0..7
    mov rcx, qword ptr [rsp + 0x30]
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz step_done
    mov r14, qword ptr [rax + 0x50]    ; the squad's own facet
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz step_done
    mov qword ptr [rsp + 0x38], rcx
    ; the slot's ammunition
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {ammo_pool_get}]
    test rax, rax
    jz step_done
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x48]
    test rax, rax
    jz step_done
    mov rcx, qword ptr [rax]
    mov rdx, qword ptr [rax + 8]
    sub rdx, rcx
    mov eax, dword ptr [rsp + 0x2c]
    imul rax, rax, 0x48
    cmp rax, rdx
    jae step_done
    add rcx, rax
    mov rax, qword ptr [rcx]
    test rax, rax
    jz step_done
    mov qword ptr [rsp + 0x40], rax    ; the weapon
    mov eax, dword ptr [rcx + 0x3c]
    mov dword ptr [rsp + 0x48], eax    ; the shared flag: nonzero disabled
    mov rcx, qword ptr [rsp + 0x38]
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz step_done                       ; not a squad: no pins
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x68]
    mov r12, qword ptr [rax]
    mov r13, qword ptr [rax + 8]
    ; marked and live members, as ammo_ui_state counts them
    mov rbp, r12
    xor esi, esi
    xor edi, edi
step_count:
    cmp rbp, r13
    jae step_counted
    call ammo_ui_next
    test rbx, rbx
    jz step_count
    inc edi
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz step_count
    inc esi
    jmp step_count
step_counted:
    cmp esi, edi
    jb step_subset
    xor esi, esi                       ; none or all marked: the whole squad
step_subset:
    mov qword ptr [rsp + 0x50], 0      ; the chosen member's facet
    mov qword ptr [rsp + 0x58], 0      ; and his AI
    mov rbp, r12
step_user:
    cmp rbp, r13
    jae step_apply
    call ammo_ui_next
    test rbx, rbx
    jz step_user
    test esi, esi
    jz step_user_guns
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz step_user
step_user_guns:
    test r15, r15
    jz step_user
    mov rcx, r15
    mov rdx, qword ptr [rsp + 0x40]
    call ammo_ui_has_weapon
    test al, al
    jz step_user
    ; his state: the pin when he has one, else the shared flag
    mov eax, dword ptr [rsp + 0x48]
    mov ecx, dword ptr [rsp + 0x2c]
    cmp byte ptr [rbx + 0x19], 0xa5
    jne step_user_state
    movzx edx, byte ptr [rbx + 0x1e]
    bt edx, ecx
    jnc step_user_state
    movzx eax, byte ptr [rbx + 0x1f]
    shr eax, cl
    and eax, 1
step_user_state:
    cmp dword ptr [rsp + 0x28], 0
    jl step_user_down
    test eax, eax
    jz step_user                       ; up: skip the enabled
    mov qword ptr [rsp + 0x50], rbx    ; the first disabled
    mov qword ptr [rsp + 0x58], r15
    jmp step_apply
step_user_down:
    test eax, eax
    jnz step_user                      ; down: skip the disabled
    mov qword ptr [rsp + 0x50], rbx    ; the last enabled so far
    mov qword ptr [rsp + 0x58], r15
    jmp step_user
step_apply:
    mov rcx, qword ptr [rsp + 0x50]
    test rcx, rcx
    jz step_done                       ; every user already there
    ; pin him as ammo-mode.asm's ammo_pin does: 1 off, 0 on
    cmp byte ptr [rcx + 0x19], 0xa5
    je step_pin
    mov byte ptr [rcx + 0x1e], 0
    mov byte ptr [rcx + 0x1f], 0
    mov byte ptr [rcx + 0x19], 0xa5
step_pin:
    mov edx, dword ptr [rsp + 0x2c]
    movzx eax, byte ptr [rcx + 0x1e]
    bts eax, edx
    mov byte ptr [rcx + 0x1e], al
    movzx eax, byte ptr [rcx + 0x1f]
    btr eax, edx
    cmp dword ptr [rsp + 0x28], 0
    jg step_pin_store
    bts eax, edx
step_pin_store:
    mov byte ptr [rcx + 0x1f], al
    cmp dword ptr [rsp + 0x28], 0
    jg step_refresh
    ; a gun keeps a loaded round of a disabled weapon and would fire it:
    ; release it, as ammo-mode.asm's ammo_unload_disabled does for pins
    mov rcx, qword ptr [rsp + 0x58]
    mov rdx, qword ptr [rsp + 0x40]
    call ammo_step_release
step_refresh:
    mov rcx, qword ptr [rsp + 0x20]
    mov rax, qword ptr [rcx]
    xorps xmm1, xmm1
    call qword ptr [rax + 0x38]        ; the menu's refresh, as its click ends
    mov al, 1
    jmp step_exit
step_done:
    xor eax, eax
step_exit:
    add rsp, 0x68
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret

; rcx menu, rdx its entity. al 1 unless another selected unit with an AI and
; ammunition is selected besides that entity and its soldiers. The player's
; selection is the manager's +40 vector: context (+128) vt+40 -> world
; (+130) vt+700(player), as SmartCursorCmdSelect finds it.
ammo_step_alone:
    push rbx
    push rsi
    push rdi
    push r12
    sub rsp, 0x28
    mov r12, rdx
    mov rbx, qword ptr [rcx + 0x130]
    mov rcx, qword ptr [rcx + 0x128]
    xor eax, eax
    test rcx, rcx
    jz alone_done
    test rbx, rbx
    jz alone_done
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x40]
    test rax, rax
    jz alone_done
    mov rdx, rax
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x700]
    test rax, rax
    jz alone_done
    mov rsi, qword ptr [rax + 0x40]
    mov rdi, qword ptr [rax + 0x48]
alone_next:
    mov al, 1
    cmp rsi, rdi
    jae alone_done
    mov rbx, qword ptr [rsi]
    add rsi, 8
    test rbx, rbx
    jz alone_next
    cmp rbx, r12
    je alone_next
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz alone_next
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz alone_next
    mov qword ptr [rsp + 0x20], rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz alone_next                      ; not selected
    mov rcx, rbx
    mov edx, 0x20
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x98]
    test al, al
    jz alone_other                     ; not a soldier
    ; one of this squad's soldiers
    mov rax, qword ptr [rsp + 0x20]
    mov rcx, qword ptr [rax + 0x50]
    mov rcx, qword ptr [rcx + 0x28]
    test rcx, rcx
    jz alone_other
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz alone_other
    cmp qword ptr [rcx + 0x10], r12
    je alone_next
alone_other:
    mov rax, qword ptr [rsp + 0x20]
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz alone_next
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {ammo_pool_get}]
    test rax, rax
    jz alone_next
    xor eax, eax
alone_done:
    add rsp, 0x28
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret

; rcx member AI, rdx weapon. Release every gun holding a loaded round of the
; weapon (Gun vt+f8), returning its reservation; no ammunition is spent.
ammo_step_release:
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    sub rsp, 0x20
    mov rbx, rcx
    mov r14, rdx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {gunner_count}]
    mov r12, rax
    xor esi, esi
release_gunner:
    cmp rsi, r12
    jae release_done
    mov rcx, rbx
    mov rdx, rsi
    inc rsi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {gunner_get}]
    test rax, rax
    jz release_gunner
    mov rdi, rax
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xf0]
    mov r13, rax
    xor ebp, ebp
release_gun:
    cmp rbp, r13
    jae release_gunner
    mov rcx, rdi
    mov rdx, rbp
    inc rbp
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xf8]
    test rax, rax
    jz release_gun
    cmp dword ptr [rax + 0xdc], 0
    je release_gun
    cmp qword ptr [rax + 0x50], r14
    jne release_gun
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xf8]
    jmp release_gun
release_done:
    add rsp, 0x20
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret

; Append unsigned eax in decimal at r10; return r10 just after the digits.
; At most ten digits; only volatile registers and private stack bytes used.
ammo_ui_number:
    sub rsp, 0x18
    lea r11, [rsp + 0x18]
    mov ecx, 10
ui_number_digit:
    xor edx, edx
    div ecx
    add dl, 0x30
    dec r11
    mov byte ptr [r11], dl
    test eax, eax
    jnz ui_number_digit
ui_number_copy:
    mov dl, byte ptr [r11]
    mov byte ptr [r10], dl
    inc r10
    inc r11
    lea rax, [rsp + 0x18]
    cmp r11, rax
    jb ui_number_copy
    add rsp, 0x18
    ret

; rcx selected entity, rdx weapon descriptor. Union of the recipients' guns,
; using the same some-marked/all-marked rule as the ammo setter. No ammo count
; or enabled flag is consulted: an empty/disabled weapon must stay toggleable.
ammo_ui_usable:
    xor r9d, r9d                     ; compatibility-only query, rdx weapon
    jmp ui_query

; rcx entity, rdx record, r8 slot. Return 0 absent, 1 all enabled,
; 2 all disabled, 3 mixed; r8d enabled users, r9d total users; xmm0 the ready
; share: the users' mean readiness (ammo_ui_ready; a disabled user counts 0),
; or -1 where the native bar stays. Count each soldier once and finish the
; scan even after discovering both states. Shared state is only an unpinned
; user's default. A unit without a squad roster (a vehicle) keeps its native
; state and count, and is one user for the share.
ammo_ui_state:
    mov r9d, 1
ui_query:
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x58
    mov dword ptr [rsp + 0x40], r9d
    mov dword ptr [rsp + 0x38], 0
    mov dword ptr [rsp + 0x3c], 0
    mov dword ptr [rsp + 0x44], 0
    mov dword ptr [rsp + 0x48], 0
    mov qword ptr [rsp + 0x28], r8
    test r9d, r9d
    jz ui_query_weapon
    mov eax, dword ptr [rdx + 0x3c]
    mov dword ptr [rsp + 0x30], eax
    mov eax, dword ptr [rdx + 0x34]
    mov dword ptr [rsp + 0x44], eax
    mov rdx, qword ptr [rdx]
ui_query_weapon:
    mov qword ptr [rsp + 0x20], rdx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz ui_usable_yes
    mov r14, qword ptr [rax + 0x50]
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz ui_usable_yes
    mov qword ptr [rsp + 0x50], rcx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz ui_single_unit                  ; not a squad: native, but for the share
    mov dword ptr [rsp + 0x44], 0
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x68]
    mov r12, qword ptr [rax]
    mov r13, qword ptr [rax + 8]
    mov rbp, r12
    xor esi, esi
    xor edi, edi
ui_count_members:
    cmp rbp, r13
    jae ui_counted_members
    call ammo_ui_next
    test rbx, rbx
    jz ui_count_members
    inc edi
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz ui_count_members
    inc esi
    jmp ui_count_members
ui_counted_members:
    cmp esi, edi
    jb ui_have_subset
    xor esi, esi
ui_have_subset:
    mov rbp, r12
ui_find_user:
    cmp rbp, r13
    jae ui_usable_no
    call ammo_ui_next
    test rbx, rbx
    jz ui_find_user
    test esi, esi
    jz ui_check_guns
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz ui_find_user
ui_check_guns:
    mov rcx, r15
    test rcx, rcx
    jz ui_find_user
    mov rdx, qword ptr [rsp + 0x20]
    call ammo_ui_has_weapon
    test al, al
    jz ui_find_user
    cmp dword ptr [rsp + 0x40], 0
    je ui_usable_yes
    mov eax, dword ptr [rsp + 0x30]
    mov rcx, qword ptr [rsp + 0x28]
    cmp rcx, 8
    jae ui_member_state
    cmp byte ptr [rbx + 0x19], 0xa5
    jne ui_member_state
    movzx edx, byte ptr [rbx + 0x1e]
    bt edx, ecx
    jnc ui_member_state
    movzx eax, byte ptr [rbx + 0x1f]
    shr eax, cl
    and eax, 1
ui_member_state:
    inc dword ptr [rsp + 0x44]       ; one soldier, regardless of gun count
    test eax, eax
    jnz ui_member_off
    inc dword ptr [rsp + 0x3c]
    mov rcx, r15
    mov rdx, qword ptr [rsp + 0x20]
    call ammo_ui_ready
    addss xmm0, dword ptr [rsp + 0x48]
    movss dword ptr [rsp + 0x48], xmm0
    mov eax, 1
    jmp ui_merge_state
ui_member_off:
    mov eax, 2
ui_merge_state:
    or dword ptr [rsp + 0x38], eax
    jmp ui_find_user                  ; keep counting after finding mixed
ui_single_unit:
    cmp dword ptr [rsp + 0x40], 0
    je ui_usable_yes                   ; compatibility query: native
    xor eax, eax
    mov dword ptr [rsp + 0x48], eax    ; disabled by the native shared flag: 0
    cmp dword ptr [rsp + 0x30], eax
    jne ui_usable_native
    mov rcx, qword ptr [rsp + 0x50]
    mov rdx, qword ptr [rsp + 0x20]
    call ammo_ui_ready
    movss dword ptr [rsp + 0x48], xmm0
    jmp ui_usable_native
ui_usable_yes:
    mov dword ptr [rsp + 0x48], 0xbf800000
ui_usable_native:
    mov eax, 1
    cmp dword ptr [rsp + 0x40], 0
    je ui_usable_done
    cmp dword ptr [rsp + 0x30], 0
    jne ui_shared_off
    mov edx, dword ptr [rsp + 0x44]
    mov dword ptr [rsp + 0x3c], edx
    jmp ui_usable_done
ui_shared_off:
    mov eax, 2                       ; non-squad: native shared flag
    jmp ui_usable_done
ui_usable_no:
    mov eax, dword ptr [rsp + 0x44]
    test eax, eax
    jz ui_share_done                   ; no users: the slot is hidden anyway
    cvtsi2ss xmm1, rax
    movss xmm0, dword ptr [rsp + 0x48]
    divss xmm0, xmm1
    movss dword ptr [rsp + 0x48], xmm0
ui_share_done:
    mov eax, dword ptr [rsp + 0x38]
ui_usable_done:
    mov r8d, dword ptr [rsp + 0x3c]
    mov r9d, dword ptr [rsp + 0x44]
    movss xmm0, dword ptr [rsp + 0x48]
    add rsp, 0x58
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret

; Advance rbp, yielding rbx selectable facet and r15 AI for a live member of
; parent r14. Preserve the caller's roster bounds and marked count.
ammo_ui_next:
    sub rsp, 0x28
    xor ebx, ebx
    xor r15d, r15d
    mov rcx, qword ptr [rbp]
    add rbp, 8
    test rcx, rcx
    jz ui_next_done
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz ui_next_done
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz ui_next_done
    cmp byte ptr [rcx + 0x18], 0
    je ui_next_done
    cmp qword ptr [rcx + 0x28], r14
    jne ui_next_done
    mov rbx, rcx
    mov r15, qword ptr [rax + 0x28]
ui_next_done:
    add rsp, 0x28
    ret

; rcx member AI, rdx descriptor. Ask every live gun's native compatibility
; method (vt+148), including alternative weapons, not only gun+50/current.
ammo_ui_has_weapon:
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    sub rsp, 0x20
    mov rbx, rcx
    mov r14, rdx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {gunner_count}]
    mov r12, rax
    xor esi, esi
ui_next_gunner:
    cmp rsi, r12
    jae ui_has_no
    mov rcx, rbx
    mov rdx, rsi
    inc rsi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {gunner_get}]
    test rax, rax
    jz ui_next_gunner
    mov rdi, rax
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xf0]
    mov r13, rax
    xor ebp, ebp
ui_next_gun:
    cmp rbp, r13
    jae ui_next_gunner
    mov rcx, rdi
    mov rdx, rbp
    inc rbp
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xf8]
    test rax, rax
    jz ui_next_gun
    mov rcx, rax
    mov rdx, r14
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x148]
    test al, al
    jz ui_next_gun
    mov eax, 1
    jmp ui_has_done
ui_has_no:
    xor eax, eax
ui_has_done:
    add rsp, 0x20
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret

; rcx member AI, rdx descriptor. Return xmm0, the member's readiness with this
; ammunition: the lowest reload progress (native vt+c8, in [0,1)) among his
; guns that have it loaded (vt+158), or 1 when none is reloading it. A ready
; gun reports 1, as the stock single-soldier bar shows.
ammo_ui_ready:
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    sub rsp, 0x30
    mov rbx, rcx
    mov r14, rdx
    mov dword ptr [rsp + 0x20], 0x3f800000
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {gunner_count}]
    mov r12, rax
    xor esi, esi
ui_ready_gunner:
    cmp rsi, r12
    jae ui_ready_done
    mov rcx, rbx
    mov rdx, rsi
    inc rsi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {gunner_get}]
    test rax, rax
    jz ui_ready_gunner
    mov rdi, rax
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xf0]
    mov r13, rax
    xor ebp, ebp
ui_ready_gun:
    cmp rbp, r13
    jae ui_ready_gunner
    mov rcx, rdi
    mov rdx, rbp
    inc rbp
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xf8]
    test rax, rax
    jz ui_ready_gun
    mov qword ptr [rsp + 0x28], rax
    mov rcx, rax
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x158]
    cmp rax, r14
    jne ui_ready_gun
    mov rcx, qword ptr [rsp + 0x28]
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xc8]
    xorps xmm1, xmm1
    comiss xmm0, xmm1
    jb ui_ready_gun                  ; negative or NaN: not a reload
    comiss xmm0, dword ptr [rsp + 0x20]
    jae ui_ready_gun                 ; not below the lowest so far
    movss dword ptr [rsp + 0x20], xmm0
    jmp ui_ready_gun
ui_ready_done:
    movss xmm0, dword ptr [rsp + 0x20]
    add rsp, 0x30
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret
