; Make a squad panel icon select the whole squad again.
;
; This is a game.dll patch, unlike everything else here, which is logic.dll.
;
; Every icon path on PlayerUnitListSquadPanel selects the entity stored in an
; icon row at row->+0x1a0, and that entity is a squad *member*. Vanilla still
; ended up selecting the squad, because selecting a member forwarded to the
; squad and isSelected on any member then answered for the squad. The
; selection patch removed that forwarding, so an icon click now selects one
; soldier, and a double click selects one soldier from each squad of a type
; rather than the squads themselves.
;
; Two entry points, both of which resolve their row through fn_1f42d0:
;
;   fn_1f4370(panel, entity, ctrl)   the single click
;   fn_1f4200(panel)                 the double click
;
; The resolver cannot see the modifier keys, which arrive in the event context
; the handlers get, so the expansion happens in the handlers instead. A plain
; click expands to the squad; Ctrl+click does not, which keeps the individual
; toggle that cross-squad subsets and control groups are built from. The world
; hooks below expand plain/Shift clicks and accepted double-click results;
; Ctrl and Ctrl+Shift keep the specific soldier under the cursor.
;
; entity: vt+0xb0 -> its facets, +0x50 -> the selectable facet
; SquadUnitSelectableFacet: +0x28 -> the squad's selectable facet
; SelectableFacet: +0x10 -> a holder whose +0x10 is the entity, which is the
;                  chain fn_417c80 walks on itself
;
; squad_of: rcx = a member entity, returns the squad's entity in rax, or the
; entity it was given if any hop is missing. Leaf: no calls except the one
; virtual, so it keeps its own shadow space.

; Every hop is also written to a trace area in the block, which the injector
; reads back with --probe. The chain below was inferred rather than measured
; and the fallback is firing in game, so the trace is how the real layout gets
; established instead of guessed at again.
; Each hop is range-checked before it is dereferenced. `+0x28` is a facet
; pointer on a soldier's facet but a flag byte on a squad's, and the two
; neighbours after it then read as a huge non-canonical "pointer" that the null
; tests happily accept. Dereferencing that is what crashed the game, so anything
; outside a plausible user address falls back instead.
squad_of:
    push rbx
    push rsi
    sub rsp, 0x28
    mov rsi, rcx                       ; keep the member as the fallback
    mov rax, rcx                       ; which also covers being handed nothing
    lea rbx, [rip + {cursor}]          ; the trace, at a fixed offset in the block
    mov qword ptr [rbx], rcx           ; +0x00 the entity handed in
    inc qword ptr [rbx + 0x38]         ; +0x38 how many times this has run
    test rcx, rcx
    jz squad_done
    mov rax, qword ptr [rcx]
    mov edx, 0x200
    call qword ptr [rax + 0x98]
    test al, al
    jnz squad_fallback                ; BuildingSelectableFacet+28 is a manager
    mov rcx, rsi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]        ; the member's facets
    mov qword ptr [rbx + 0x08], rax    ; +0x08
    test rax, rax
    jz squad_fallback
    mov rax, qword ptr [rax + 0x50]    ; his selectable facet
    mov qword ptr [rbx + 0x10], rax    ; +0x10
    mov rdx, 0x800000000000            ; a user pointer is below this
    test rax, rax
    jz squad_fallback
    cmp rax, rdx
    jae squad_fallback
    cmp rax, 0x10000
    jb squad_fallback
    mov rax, qword ptr [rax + 0x28]    ; the squad's selectable facet
    mov qword ptr [rbx + 0x18], rax    ; +0x18
    test rax, rax
    jz squad_fallback
    cmp rax, rdx
    jae squad_fallback
    cmp rax, 0x10000
    jb squad_fallback
    mov rax, qword ptr [rax + 0x10]    ; its holder, as fn_417c80 walks its own
    mov qword ptr [rbx + 0x20], rax    ; +0x20
    test rax, rax
    jz squad_fallback
    cmp rax, rdx
    jae squad_fallback
    cmp rax, 0x10000
    jb squad_fallback
    mov rax, qword ptr [rax + 0x10]    ; the squad entity, in theory
    mov qword ptr [rbx + 0x28], rax    ; +0x28
    test rax, rax
    jnz squad_done
squad_fallback:
    mov rax, rsi
squad_done:
    mov qword ptr [rbx + 0x30], rax    ; +0x30 what it actually returned
    add rsp, 0x28
    pop rsi
    pop rbx
    ret

; ---------------------------------------------------------------------------
; The single click. fn_1f4370's first two instructions are displaced:
;   mov qword ptr [rsp + 8], rbx     48 89 5c 24 08
;   mov qword ptr [rsp + 0x10], rsi  48 89 74 24 10
; Ten bytes, so a five-byte jmp fits with five to spare. rcx is the panel, rdx
; the entity, r8b the Ctrl flag; all are still live here, and rax is free.
single_click:
    mov qword ptr [rsp + 8], rbx       ; the two displaced stores, verbatim
    mov qword ptr [rsp + 0x10], rsi
    lea r11, [rip + {cursor}]          ; r11 is volatile and this is the entry
    inc qword ptr [r11 + 0x40]         ; +0x40 how often the hook was reached
    mov qword ptr [r11 + 0x48], r8     ; +0x48 the Ctrl flag it was given
    test r8b, r8b
    jnz single_ctrl                    ; Ctrl held: ignore actual squad rows
    push rcx
    push rdx
    push r8
    sub rsp, 0x20
    mov rcx, rdx
    call squad_of
    add rsp, 0x20
    pop r8
    pop rdx
    pop rcx
    mov rdx, rax                       ; select the squad instead
    jmp single_tail
single_ctrl:
    push rcx
    push rdx
    push r8
    sub rsp, 0x20
    mov rcx, rdx
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]
    add rsp, 0x20
    pop r8
    pop rdx
    pop rcx
    test al, al
    jz single_tail
    ret
single_tail:
    ; the block is allocated, so a jump back into the module cannot be a baked
    ; rel32. The injector fills these imm64s with the module base plus an rva.
    mov r11, 0xaaaaaaaaaaaaaaa1        ; fixup: resume fn_1f4370 + 0xa
    jmp r11

; ---------------------------------------------------------------------------
; The double click. fn_1f4200 resolves its row and keeps the entity in rax and
; rsi. Its first three instructions are displaced:
;   mov qword ptr [rsp + 0x10], rsi  48 89 74 24 10
;   push rdi                         57
;   sub rsp, 0x40                    48 83 ec 40
; Ten bytes again. The expansion has to happen after fn_1f42d0 returns, so
; this re-runs the prologue, calls the resolver itself, expands, and rejoins
; past both the call and the two stores of its result.
double_click:
    mov qword ptr [rsp + 0x10], rsi
    lea r11, [rip + {cursor}]
    inc qword ptr [r11 + 0x50]         ; +0x50 how often the double click ran
    push rdi
    sub rsp, 0x40
    mov rdi, rcx                       ; as the original does
    call control_down
    test ax, ax
    jns double_resolve
    xor eax, eax
    jmp double_tail
double_resolve:
    mov rcx, rdi
    mov r11, 0xaaaaaaaaaaaaaaa2        ; fixup: fn_1f42d0, the row resolver
    call r11
    test rax, rax
    jz double_tail                     ; nothing under the cursor
    mov rcx, rax
    call squad_of
double_tail:
    mov rsi, rax
    mov r11, 0xaaaaaaaaaaaaaaa3        ; fixup: resume fn_1f4200 + 0x15
    jmp r11

; ---------------------------------------------------------------------------
; The world cursor's plain select, the squad half of the mode. Shift is the
; engine's own modifier: SmartCursorCmdSelect branches on a Shift flag, and the
; zero path clears then calls the manager's select (vt+0x60) on the entity under
; the cursor. That call, at game.dll+0x3325dc, is the squad one, so this expands
; the entity with squad_of and redoes it. Ctrl is read here, at the click,
; because the command carries only the Shift bit; when it is down the entity is
; selected as it stands, which is the one soldier.
;
; The call is only three bytes, too short for the jump on its own, so the mov
; after it is displaced too and both are redone on the way out. rcx is the
; manager, rax its vtable, rdx the entity. GetAsyncKeyState clobbers the
; volatile registers, so the three are saved and rdx is restored or replaced.
world_select:
    push rcx
    push rdx
    push rax
    sub rsp, 0x28
    mov ecx, 0x11                      ; VK_CONTROL
    mov rax, 0xaaaaaaaaaaaaaaa6        ; fixup: user32!GetAsyncKeyState
    call rax
    add rsp, 0x28
    test ax, ax
    js ws_keep                         ; Ctrl down: keep the one soldier
    mov rcx, qword ptr [rsp + 8]       ; the entity, which the call clobbered
    sub rsp, 0x28
    call squad_of
    add rsp, 0x28
    mov rdx, rax
    jmp ws_ready
ws_keep:
    mov rdx, qword ptr [rsp + 8]
ws_ready:
    pop rax                            ; the manager's vtable
    pop r11                            ; the old entity, discarded
    pop rcx                            ; the manager
    call qword ptr [rax + 0x60]        ; the displaced call, redone
    mov rbx, qword ptr [rsp + 0x30]    ; the displaced mov, redone
    mov r11, 0xaaaaaaaaaaaaaaa4        ; fixup: resume game.dll+0x3325e4
    jmp r11

; ---------------------------------------------------------------------------
; The world cursor's toggle, the other squad half; Shift+click. It toggles the
; squad, as vanilla did through the member's forwarding, so it expands the same
; way before the tail jmp into vt+0x70. Ctrl+Shift leaves the one soldier.
world_toggle:
    push rbx
    push rsi
    push rax
    push rcx
    push rdx
    sub rsp, 0x28
    mov ecx, 0x11                      ; VK_CONTROL
    mov rax, 0xaaaaaaaaaaaaaaa8        ; fixup: user32!GetAsyncKeyState
    call rax
    add rsp, 0x28
    test ax, ax
    js wt_keep                         ; Ctrl down: toggle the one soldier
    mov rcx, qword ptr [rsp]           ; the entity, which the call clobbered
    sub rsp, 0x28
    call squad_of
    add rsp, 0x28
    mov r10, rax
    jmp wt_ready
wt_keep:
    mov r10, qword ptr [rsp]
wt_ready:
    pop rdx
    pop rcx
    pop rax
    pop rsi
    pop rbx
    mov rdx, r10                       ; the squad, or the entity unchanged
    mov rbx, qword ptr [rsp + 0x30]    ; the displaced mov, redone
    mov r11, 0xaaaaaaaaaaaaaaa5        ; fixup: resume game.dll+0x3325b0
    jmp r11

; ---------------------------------------------------------------------------
; The smart cursor's select rule, which Ctrl has to get past. fn_352af0 builds
; the left-click command, and at 0x352de6 it counts the chooser's selection
; ([rsi+0x98, rsi+0xa0)). With at most one entry, a hovered entity that is
; already selected, or whose group AiUtils vt+0x840 finds selected, gets no
; command at all: clicking the one selected squad does nothing. The cursor
; cannot see Ctrl, so Ctrl+click and Ctrl+Shift+click on that squad's
; soldiers were swallowed too, and a soldier could be neither picked out of
; nor removed from the only selected squad. Plain clicks now always reach the
; builder to complete partial selection. Ctrl also reaches it for individual
; hits, but actual squad containers go to the no-op command before any clear.
;
; rsi is the chooser; rax, rcx, rdx and r8-r11 are dead here, since both
; destinations reload what they use. rsp is 16-byte aligned in fn_352af0's
; body and the hook is a jmp, so 0x20 of shadow space keeps the call aligned.
ctrl_select:
    sub rsp, 0x20
    call control_down
    add rsp, 0x20
    test ax, ax
    jns cs_select                      ; plain click always completes a subset
    ; RDI is the already-validated selectable facet. Resolve its entity and
    ; reject squad-icon hits before a command can clear or toggle selection.
    mov rcx, qword ptr [rdi + 0x10]
    test rcx, rcx
    jz cs_ignore
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz cs_ignore
    sub rsp, 0x20
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]
    add rsp, 0x20
    test al, al
    jz cs_select
cs_ignore:
    mov r11, 0xaaaaaaaaaaaaaaab        ; fixup: no selection command
    jmp r11
cs_select:
    mov r11, 0xaaaaaaaaaaaaaaac        ; fixup: fn_352af0's select builder, 0x352e3f
    jmp r11

; Tail call retains the caller's shadow space and returns the key-state AX.
control_down:
    mov ecx, 0x11
    mov rax, 0xaaaaaaaaaaaaaaad
    jmp rax

; World double-click: retain the stock hit entity and eligibility query.
; RDI is the hit entity, R14 the manager, and [rsp+0x20] the screen region.
; Squad containers do not necessarily pass world-object eligibility. Expand
; the selected results only AFTER matching the actual soldiers in the world.
world_double:
    sub rsp, 0x20
    call control_down
    add rsp, 0x20
    test ax, ax
    js wd_resume                      ; Ctrl double-click must not expand squads
    mov r8, rdi
    lea rdx, [rsp + 0x20]
    mov rcx, r14
    mov rax, qword ptr [r14]
    call qword ptr [rax + 0x80]
    ; Preserve the enclosing function's nonvolatile registers. Selection does
    ; not change the registry; retain its bounds outside outgoing shadow space.
    push rbx
    push rsi
    sub rsp, 0x30
    mov rbx, qword ptr [r14 + 0x28]
    mov rax, qword ptr [r14 + 0x30]
    mov qword ptr [rsp + 0x20], rax
wd_next:
    cmp rbx, qword ptr [rsp + 0x20]
    jae wd_done
    mov rsi, qword ptr [rbx]
    add rbx, 8
    mov rcx, rsi
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz wd_next
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz wd_next
    mov rcx, rsi
    call squad_of
    cmp rax, rsi
    je wd_next                         ; standalone entity already selected
    mov rdx, rax
    mov rcx, r14
    mov rax, qword ptr [r14]
    call qword ptr [rax + 0x60]        ; mark the complete accepted squad
    jmp wd_next
wd_done:
    add rsp, 0x30
    pop rsi
    pop rbx
wd_resume:
    mov r11, 0xaaaaaaaaaaaaaaa9        ; fixup: resume game.dll+0x348fb8
    jmp r11

; AmmunitionMenu::fill: r12 selected entity, rbp slot, rsi shared record,
; rdi menu. Aggregate only users of this weapon and pass fillSlot a private
; record with a valid binary flag; decorate mixed after the stock draw.
ammo_panel:
    sub rsp, 0x90
    mov rcx, r12
    mov rdx, rsi
    mov r8, rbp
    call ammo_ui_state
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
    imul rax, rbp, 0xb8
    lea r10, [rdi + rax + 0x180]
    mov rcx, qword ptr [r10 + 0x40]
    mov qword ptr [rsp + 0x78], rcx
    cmp dword ptr [rsp + 0x68], 3
    jne ammo_panel_count
    mov dword ptr [r10 + 8], 1        ; next click enables all selected users
    mov rcx, qword ptr [r10 + 0x30]
    test rcx, rcx
    jz ammo_panel_count
    mov dword ptr [rcx + 0x1a0], 0xffffc04d
    mov byte ptr [rcx + 0x188], 1
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
; 2 all disabled, 3 mixed; r8d enabled users, r9d total users. Count each
; soldier once and finish the scan even after discovering both states.
; Shared state is only an unpinned user's default.
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
    sub rsp, 0x48
    mov dword ptr [rsp + 0x40], r9d
    mov dword ptr [rsp + 0x38], 0
    mov dword ptr [rsp + 0x3c], 0
    mov dword ptr [rsp + 0x44], 0
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
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz ui_usable_yes                   ; non-squad panels retain native behavior
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
    mov eax, 1
    jmp ui_merge_state
ui_member_off:
    mov eax, 2
ui_merge_state:
    or dword ptr [rsp + 0x38], eax
    jmp ui_find_user                  ; keep counting after finding mixed
ui_usable_yes:
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
    mov eax, dword ptr [rsp + 0x38]
ui_usable_done:
    mov r8d, dword ptr [rsp + 0x3c]
    mov r9d, dword ptr [rsp + 0x44]
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
