; Passenger mounts copy one source gun from their occupant. The native chooser
; returns the first eligible gun, which can be the standard gun before a
; special gun. A non-usual weapon_type marks the special gun mounts in the
; game's squad data. Prefer such a gun for this bind; retain the native answer
; when the occupant has no non-grenade special gun.

vehicle_special_fire_source:
    push rbx
    push rsi
    push rdi
    sub rsp, 0x20
    mov rbx, rcx
    call {vehicle_special_fire_original}
    mov rsi, rax
    test rsi, rsi
    jz vehicle_special_fire_done
    mov rbx, qword ptr [rbx + 0x1f0]
    test rbx, rbx
    jz vehicle_special_fire_done
    mov rbx, qword ptr [rbx + 0x10]
    test rbx, rbx
    jz vehicle_special_fire_done
    mov rdi, qword ptr [rbx + 0x40]
    mov rbx, qword ptr [rbx + 0x38]
vehicle_special_fire_next:
    cmp rbx, rdi
    jae vehicle_special_fire_done
    mov rax, qword ptr [rbx]
    test rax, rax
    jz vehicle_special_fire_advance
    cmp rax, rsi
    je vehicle_special_fire_advance
    mov rcx, qword ptr [rax]
    cmp rcx, qword ptr [rsi]
    jne vehicle_special_fire_advance
    mov rcx, qword ptr [rax + 0x40]
    test rcx, rcx
    jz vehicle_special_fire_advance
    cmp dword ptr [rcx + 0x108], 0
    je vehicle_special_fire_advance
    cmp byte ptr [rcx + 0xa9], 0
    jne vehicle_special_fire_advance
    mov rsi, rax
    jmp vehicle_special_fire_done
vehicle_special_fire_advance:
    add rbx, 8
    jmp vehicle_special_fire_next
vehicle_special_fire_done:
    mov rax, rsi
    add rsp, 0x20
    pop rdi
    pop rsi
    pop rbx
    ret

; The gunner may move a passenger binding between mount positions each tick.
; Source guns can be destroyed while their dummy mount still holds a raw pointer.
; Release that binding before the move dereferences a former Gun object.
vehicle_special_fire_rebind_guard:
    test esi, esi
    js vehicle_special_fire_guard_skip
    movsxd r13, r14d
    mov rax, qword ptr [rbx + 0x68]
    mov rdx, qword ptr [rax + r13*8]
    test rdx, rdx
    jz vehicle_special_fire_guard_release
    mov rax, qword ptr [rdx]
    mov rcx, qword ptr [rbx + 0x20]
    mov rcx, qword ptr [rcx + r13*8]
    cmp rax, qword ptr [rcx]
    je vehicle_special_fire_guard_resume
vehicle_special_fire_guard_release:
    mov rcx, qword ptr [rbx + 0x20]
    mov rcx, qword ptr [rcx + r13*8]
    mov rax, qword ptr [rcx]
    xor edx, edx
    sub rsp, 0x20
    call qword ptr [rax + 0x60]
    add rsp, 0x20
    mov rcx, qword ptr [rbx + 0x68]
    mov qword ptr [rcx + r13*8], 0
vehicle_special_fire_guard_skip:
    jmp {vehicle_special_fire_guard_skip}
vehicle_special_fire_guard_resume:
    jmp {vehicle_special_fire_guard_resume}
