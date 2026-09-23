; Replace only fn_418cb0's squad branch, [0x418cda, 0x418d56).
; The entry has already tested entity kind 0x10. RBX is the squad entity.
; Retain the original prologue/unwind record and the non-squad path.
; The old branch copied the member vector then selected only its first item.
; Visit every member instead, using RBX (already saved) for the iterator and
; [rsp+0x20] for the end, outside the 32-byte outgoing shadow space.
mov rax, qword ptr [rbx]
mov rcx, rbx
call qword ptr [rax + 0xb0]
mov rcx, qword ptr [rax + 0x28]
test rcx, rcx
jz 0x418da7
mov rax, qword ptr [rcx]
call qword ptr [rax + {squad_roster}]
test rax, rax
jz 0x418da7
mov rdx, qword ptr [rax]
mov rcx, rax
call qword ptr [rdx + 0x68]
mov rbx, qword ptr [rax]
mov rax, qword ptr [rax + 8]
mov qword ptr [rsp + 0x20], rax
next:
cmp rbx, qword ptr [rsp + 0x20]
jae 0x418da7
mov rcx, qword ptr [rbx]
add rbx, 8
test rcx, rcx
jz next
mov rax, qword ptr [rcx]
call qword ptr [rax + 0xb0]
mov rcx, qword ptr [rax + 0x50]
test rcx, rcx
jz next
mov rax, qword ptr [rcx]
mov dl, 1
call qword ptr [rax + 0x50]
jmp next
