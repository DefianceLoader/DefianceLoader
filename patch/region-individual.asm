; Marquee eligibility wrapper: which entities a dragged box selects. Keeps the
; native ownership, spatial and active checks. RCX entity, RDX region, R8
; context. Called from four region-manager sites, including Shift's deselect
; pass. Same-type double-click is not hooked.
;
; The cell ({scratch}) holds GetAsyncKeyState at +0 and the mode byte at +8,
; both written by Core when selection installs (`marquee` in infantry.ini):
;
;   0 soldiers  the individual soldiers inside; squad containers never count
;   1 squads    the base game's box: a squad counts when its icon or any of
;               its soldiers is inside, and soldiers never count on their own,
;               so the whole squad is selected; with Ctrl held, as soldiers
;
; A squad selected through the manager marks its whole roster
; (patch/select-squad.asm), so squads mode leaves no subset behind.
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
    call qword ptr [rax + 0x98]
    test al, al
    jnz marquee_reject                 ; a soldier: his squad counts instead
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]
    test al, al
    jz marquee_native                  ; not a squad: the stock test
    mov rcx, rbx
    mov rdx, rsi
    mov r8, rdi
    call native_eligible               ; the squad's icon or position
    test al, al
    jnz marquee_done
    ; any soldier inside: squad entity vt+0xb0 -> facets +0x28 -> its roster
    ; -> vt+0x68, the members' vector, as patch/select-squad.asm walks it
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz marquee_reject
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz marquee_reject
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz marquee_reject
    mov rcx, rax
    mov rdx, qword ptr [rax]
    call qword ptr [rdx + 0x68]
    mov rbx, qword ptr [rax]
    mov r12, qword ptr [rax + 8]
marquee_member:
    cmp rbx, r12
    jae marquee_reject
    mov rcx, qword ptr [rbx]
    add rbx, 8
    test rcx, rcx
    jz marquee_member
    mov rdx, rsi
    mov r8, rdi
    call native_eligible               ; his own position, not the icon
    test al, al
    jz marquee_member
    mov eax, 1
    jmp marquee_done

marquee_soldiers:
    mov rcx, rbx
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]
    test al, al
    jnz marquee_reject                 ; a squad container
marquee_native:
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

; The marquee's replace pass (fn_418db0) selects each hit through its own
; setSelected (0x418e90). For a squad that selects the squad but marks none of
; its soldiers, so their selection halos stay off; the manager's select,
; fn_418cb0, marks the whole roster (patch/select-squad.asm). Called over that
; loop body with RBX the hit's slot: a squad goes through fn_418cb0 (which does
; not use RCX), anything else keeps the stock setSelected(1). RBX is kept.
marquee_select:
    sub rsp, 0x28
    mov rcx, qword ptr [rbx]
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]
    test al, al
    jz marquee_select_stock
    mov rdx, qword ptr [rbx]
    xor ecx, ecx
    call 0x418cb0
    jmp marquee_select_done
marquee_select_stock:
    mov rcx, qword ptr [rbx]
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz marquee_select_done
    mov rax, qword ptr [rcx]
    mov dl, 1
    call qword ptr [rax + 0x50]
marquee_select_done:
    add rsp, 0x28
    ret

; The native predicate, fn_418000(entity, region, context). Every marquee test
; goes through this one call, so region_icon_gate knows it by its return point.
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
    call qword ptr [rax + 0x98]
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
