; Prototype only: not installed by build.py or payload.py.
; Compact a command-local vector BEFORE formation destinations are allocated.
; Windows x64: RCX=begin, RDX=end, R8B=subset mode, R9=soldier facet vtable.
; Return the new end pointer in RAX. Caller retains ownership/capacity and
; must update its local vector's end. Squad mode leaves the vector untouched.
; Input contains live entity pointers from the engine's member collection;
; nulls are tolerated, dangling pointers are not. This filters membership,
; not ownership, death, pathability, or AI eligibility. No global state.
; Empty subsets remain empty. Duplicate input entries remain duplicates.
; Five nonvolatile pushes + 32-byte shadow space align the virtual call.

test r8b, r8b
jnz subset
mov rax, rdx
ret
subset:
push rbx
push rbp
push rsi
push rdi
push r12
sub rsp, 0x20
mov rsi, rcx
mov rdi, rcx
mov rbp, rdx
mov r12, r9
next:
cmp rsi, rbp
jae done
mov rbx, qword ptr [rsi]
add rsi, 8
test rbx, rbx
jz next
mov rcx, rbx
mov rax, qword ptr [rcx]
call qword ptr [rax + 0xb0]
test rax, rax
jz next
mov rax, qword ptr [rax + 0x50]
test rax, rax
jz next
cmp qword ptr [rax], r12
jne next
; Use the getter so a deselected parent squad also excludes this soldier.
mov rcx, rax
mov rax, qword ptr [rax]
call qword ptr [rax + 0x58]
test al, al
jz next
mov qword ptr [rdi], rbx
add rdi, 8
jmp next
done:
mov rax, rdi
add rsp, 0x20
pop r12
pop rdi
pop rsi
pop rbp
pop rbx
ret
