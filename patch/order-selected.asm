; The shared member filter of patch/order-attack.asm and patch/order-garrison.asm.
; Each unit assembles its own copy; they share no state.
;
; rcx = borrowed command-local array, rdx = count; returns retained count.
; No marks means stock squad behavior. A subset keeps marked members in order.
; Never modify the original squad vector, allocate memory, or retain pointers.
selected_members:
    xor r8d, r8d
filter_members:
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    push r14
    sub rsp, 0x28
    mov r14d, r8d
    mov rsi, rcx
    mov r12, rdx
    xor ebx, ebx
    xor edi, edi
count_members:
    cmp rbx, r12
    jae counted_members
    mov rcx, qword ptr [rsi + rbx*8]
    call member_selected
    movzx eax, al
    add rdi, rax
    inc rbx
    jmp count_members
counted_members:
    test rdi, rdi
    jz keep_members
    cmp rdi, r12
    je keep_members
    xor ebx, ebx
    xor edi, edi
pack_members:
    cmp rbx, r12
    jae packed_members
    mov r13, qword ptr [rsi + rbx*8]
    mov rcx, r13
    call member_selected
    test al, al
    jz left_behind_member
    mov qword ptr [rsi + rdi*8], r13
    inc rdi
next_member:
    inc rbx
    jmp pack_members
left_behind_member:
    test r14d, r14d
    jz next_member
    test r13, r13
    jz next_member
    mov rcx, r13
    mov rax, qword ptr [rcx]
    mov edx, 0x20
    call qword ptr [rax + 0x98]       ; a soldier
    test al, al
    jz next_member
    mov rcx, r13
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz next_member
    mov rcx, qword ptr [rax + 0x58]   ; posture
    test rcx, rcx
    jz next_member
    cmp dword ptr [rcx + 0x74], 3
    jne next_member                   ; not lying prone
    mov rcx, qword ptr [rax + 0x50]   ; selectable facet
    test rcx, rcx
    jz next_member
    mov byte ptr [rcx + 0x31], 3
    mov word ptr [rcx + 0x32], 0x7a5e
    jmp next_member
keep_members:
    mov rdi, r12
packed_members:
    mov rax, rdi
    add rsp, 0x28
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret

; Use the selection plugin's effective getter, including enabled/squad state.
member_selected:
    sub rsp, 0x28
    test rcx, rcx
    jz no_selection
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz no_selection
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz no_selection
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    add rsp, 0x28
    ret
no_selection:
    xor eax, eax
    add rsp, 0x28
    ret
