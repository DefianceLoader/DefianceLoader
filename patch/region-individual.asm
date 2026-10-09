; Marquee eligibility wrapper: which entities a dragged box selects. Keeps the
; native ownership, spatial and active checks. RCX entity, RDX region, R8
; context. Called from four region-manager sites, including Shift's deselect
; pass. Same-type double-click is not hooked.
;
; The cell ({scratch}) holds GetAsyncKeyState at +0 and the mode byte at +8,
; both written by Core when selection installs (`marquee` in infantry.ini):
;
;   0 soldiers  the individual soldiers inside; squad containers never count
;   1 squads    the base game's box: a squad counts when it or any of its
;               soldiers passes the stock predicate, and soldiers never count
;               on their own; a soldier whose selection is disabled (aboard
;               a vehicle) does not count for his squad; with Ctrl held, as
;               soldiers
;
; The base game reaches the same squads through the soldier's setter, which
; forwards to his squad. Selection replaces that setter with one that marks
; only the soldier, so squads mode hands the box squads instead, and
; marquee_select selects them through the manager, which marks the whole
; roster (patch/select-squad.asm). Members are tested by the stock predicate,
; icon shortcut included, as the base game tests its soldiers.
region_individual:
    push rbx
    push rsi
    push rdi
    push r12
    sub rsp, 0x28
    mov rbx, rcx
    mov rsi, rdx
    mov rdi, r8
    lea r12, [rip + {scratch}]
    cmp byte ptr [r12 + 8], 1
    jne marquee_soldiers
    mov rax, qword ptr [r12]
    test rax, rax
    jz marquee_squads
    mov ecx, 0x11                      ; VK_CONTROL
    call rax
    test ax, ax
    js marquee_soldiers                ; Ctrl held: individuals

marquee_squads:
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    mov edx, 0x20
    call qword ptr [rax + 0x98]    ; vt:essence/EntityImpl@Essence@Galileo
    test al, al
    jnz marquee_reject                 ; a soldier: his squad counts instead
    mov rcx, rbx
    mov rdx, rsi
    mov r8, rdi
    call 0x418000                     ; the stock test, for squads and the rest
    test al, al
    jnz marquee_done
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]    ; vt:essence/EntityImpl@Essence@Galileo
    test al, al
    jz marquee_reject                  ; not a squad, and the stock test refused
    ; any soldier the stock test takes: squad entity vt+0xb0 -> facets +0x28
    ; -> its roster -> vt+0x68, the members' vector, as patch/select-squad.asm
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]    ; vt:essence/EntityImpl@Essence@Galileo
    test rax, rax
    jz marquee_reject
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz marquee_reject
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]    ; vt:logic/SquadAiFacet@Leonardo
    test rax, rax
    jz marquee_reject
    mov rcx, rax
    mov rdx, qword ptr [rax]
    call qword ptr [rdx + 0x68]    ; vt:logic/SquadHolderFacet@Leonardo
    mov rbx, qword ptr [rax]
    mov r12, qword ptr [rax + 8]
marquee_member:
    cmp rbx, r12
    jae marquee_reject
    mov rcx, qword ptr [rbx]
    add rbx, 8
    test rcx, rcx
    jz marquee_member
    ; a member counts only where the replace pass would test him: it skips an
    ; entity whose selectable facet (facets +0x50) is missing or disabled
    ; (+0x18 zero; fn_418db0, 0x418e10), as a crew aboard a vehicle is. Boxing
    ; the vehicle selects the vehicle, and his squad, selected too, would take
    ; the move order and dismount.
    mov qword ptr [rsp + 0x20], rcx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]    ; vt:essence/EntityImpl@Essence@Galileo
    mov rax, qword ptr [rax + 0x50]
    test rax, rax
    jz marquee_member
    cmp byte ptr [rax + 0x18], 0
    je marquee_member
    mov rcx, qword ptr [rsp + 0x20]
    mov rdx, rsi
    mov r8, rdi
    call 0x418000
    test al, al
    jz marquee_member
    mov eax, 1
    jmp marquee_done

marquee_soldiers:
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]    ; vt:essence/EntityImpl@Essence@Galileo
    test al, al
    jnz marquee_reject                 ; a squad container
    mov rcx, rbx
    mov rdx, rsi
    mov r8, rdi
    call native_eligible
    jmp marquee_done
marquee_reject:
    xor eax, eax
marquee_done:
    add rsp, 0x28
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret

; The box's replace pass (fn_418db0, 0x418e90) calls each hit's own
; setSelected(1). For a squad that selects the squad but marks none of its
; soldiers, so their selection halos stay off; the manager's select, fn_418cb0,
; marks the whole roster (patch/select-squad.asm). Shift's add already goes
; through fn_418cb0 (tools/build.py, select_toggle). Called over the replace
; pass's loop body with RBX the hit's slot:
; a squad goes through fn_418cb0 (which does not use RCX), anything else keeps
; the stock setSelected(1). RBX is kept.
marquee_select:
    sub rsp, 0x28
    mov rcx, qword ptr [rbx]
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]    ; vt:essence/EntityImpl@Essence@Galileo
    test al, al
    jz marquee_select_stock
    mov rdx, qword ptr [rbx]
    xor ecx, ecx
    call 0x418cb0
    jmp marquee_select_done
marquee_select_stock:
    mov rcx, qword ptr [rbx]
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]    ; vt:essence/EntityImpl@Essence@Galileo
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz marquee_select_done
    mov rax, qword ptr [rcx]
    mov dl, 1
    call qword ptr [rax + 0x50]    ; vt:logic/SelectableFacet@Leonardo
marquee_select_done:
    add rsp, 0x28
    ret

; The native predicate, fn_418000(entity, region, context). The individual path
; goes through this call, so region_icon_gate knows it by its return point.
native_eligible:
    sub rsp, 0x28
    call 0x418000
region_native_return:
    add rsp, 0x28
    ret
region_reject:                         ; spacing: region_icon_gate's lea counts on it
    xor eax, eax
    ret

; The native predicate accepts a hover-icon rectangle before projecting the
; entity's position. Infantry may resolve to their shared squad icon here.
; Only our marquee caller bypasses that shortcut for infantry; double-click,
; buildings and vehicles retain the stock predicate. Native prologue: three
; pushes and 0xa0 local bytes place its return address at rsp+0xb8.
region_icon_gate:
    lea rax, [rip - 0xf]           ; region_native_return (verified by native test)
    cmp qword ptr [rsp + 0xb8], rax
    jne region_icon_stock
    sub rsp, 0x20
    mov rcx, rdi
    mov rax, qword ptr [rcx]
    mov edx, 0x20
    call qword ptr [rax + 0x98]    ; vt:essence/EntityImpl@Essence@Galileo
    add rsp, 0x20
    test al, al
    jnz region_position_only
region_icon_stock:
    mov rax, qword ptr [r14]
    mov rcx, r14
    jmp 0x41812e
region_position_only:
    jmp 0x4181d3

; Preserve native building-only selection. Selecting occupants through their
; setters also selects parent squads and pollutes the building command context.
; Building exit orders already enumerate actual occupants independently of marks.
; Keep this existing verified entry stable; only the native selected flag changes.
building_select:
    mov byte ptr [rcx + 0x30], dl
    ret
