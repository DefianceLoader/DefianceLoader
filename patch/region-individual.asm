; Marquee-only eligibility wrapper. Keep native ownership, spatial and active
; checks for individual entities; squad containers never count as box hits.
; RCX entity, RDX region, R8 context. Called from four region-manager sites,
; including Shift's deselect pass. Same-type double-click is not hooked.
region_individual:
    push rcx
    push rdx
    push r8
    sub rsp, 0x20
    mov rax, qword ptr [rcx]
    mov edx, 0x10
    call qword ptr [rax + 0x98]
    add rsp, 0x20
    pop r8
    pop rdx
    pop rcx
    test al, al
    jnz region_reject
    sub rsp, 0x28
    call 0x418000
region_native_return:
    add rsp, 0x28
    ret
region_reject:
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
