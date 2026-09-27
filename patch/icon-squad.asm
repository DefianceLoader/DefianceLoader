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
