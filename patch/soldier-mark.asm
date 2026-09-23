; SquadUnitSelectableFacet::setSelected, the soldier's, moved out of line.
;
; Entered by a jmp laid over the whole stock function at logic.dll 0x4491a0,
; so this *is* that function: rcx the soldier's selectable facet, dl the value,
; the caller's return address on the stack.
;
;   soldier facet  +0x18 enabled, +0x28 his squad's facet, +0x30 his own mark
;   squad facet    +0x10 a holder whose +0x10 is the squad entity, vt+0x50
;                  setSelected, vt+0x58 isSelected
;
; A soldier's mark says he is picked; the squad's flag says the squad is
; selected at all, which the panel and the orders are built on. Writing only
; the mark, as the in-place rewrite did, let the two disagree in two ways:
;
; - Unmarking the last marked soldier left his squad selected with nobody
;   marked. Nothing showed as selected, the squad still took orders, and the
;   move filter reads "nobody marked" as no preference, so all of it moved.
;   Now the squad is deselected along with its last mark.
; - Deselecting a squad as a whole writes only the squad's flag, so its
;   soldiers keep their marks. Marking one of them later brought those stale
;   marks back with him. A squad that is not selected has no live marks, so
;   they are cleared before his is set.
;
; Marking still selects the squad through its own setSelected, and a soldier
; without a squad has only his own mark. Other soldiers' marks are written
; directly, never through their setters, so this cannot re-enter itself. A
; squad facet's +0x28 is a flag byte, never this pointer, so matching a
; member's +0x28 against the squad facet also passes over anything that is not
; a soldier of this squad. If the members cannot be listed, unmarking keeps
; the squad selected, which is what the in-place rewrite did.
;
; The prologue is the shape tools/build.py describes in its unwind record.

    push rbx
    push rsi
    push rdi
    push r12
    push r13
    push r14
    sub rsp, 0x28
    mov rbx, rcx                       ; the soldier's facet
    movzx edi, dl                      ; the value
    mov rsi, qword ptr [rcx + 0x28]    ; his squad's facet
    mov rax, 0x800000000000            ; a user pointer is below this
    cmp rsi, 0x10000
    jb own_only                        ; not a facet at all: only his own mark
    cmp rsi, rax
    jae own_only
    cmp qword ptr [rsi], 0
    jz own_only                        ; his squad facet is gone: only his own mark
    mov rcx, rsi
    mov rax, qword ptr [rsi]
    call qword ptr [rax + 0x58]        ; is his squad selected, as isSelected asks it
    test al, al
    jnz squad_selected
    test edi, edi
    jz own_only                        ; unmarking in a deselected squad: nothing else to do
    jmp walk                           ; marking into it: clear the stale marks first
squad_selected:
    test edi, edi
    jnz mark                           ; marking into a selected squad: the marks are live

walk:
    mov r14d, 1                        ; until the members are listed, assume one is marked
    mov rax, qword ptr [rsi + 0x10]
    test rax, rax
    jz walked
    mov rcx, qword ptr [rax + 0x10]    ; the squad entity
    test rcx, rcx
    jz walked
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]        ; its facets
    test rax, rax
    jz walked
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz walked
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz walked
    mov rdx, qword ptr [rax]
    mov rcx, rax
    call qword ptr [rdx + 0x68]        ; the member vector, as select-squad.asm reads it
    mov r12, qword ptr [rax]
    mov r13, qword ptr [rax + 8]
    xor r14d, r14d
next:
    cmp r12, r13
    jae walked
    mov rcx, qword ptr [r12]
    add r12, 8
    test rcx, rcx
    jz next
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz next
    mov rcx, qword ptr [rax + 0x50]    ; the member's selectable facet
    test rcx, rcx
    jz next
    cmp rcx, rbx
    je next                            ; not the soldier being set
    cmp qword ptr [rcx + 0x28], rsi
    jne next                           ; not a soldier of this squad
    test edi, edi
    jz check
    mov byte ptr [rcx + 0x30], 0       ; stale: his squad is not selected
    jmp next
check:
    cmp byte ptr [rcx + 0x30], 0
    je next
    cmp byte ptr [rcx + 0x18], 0
    je next                            ; a mark counts while enabled, as isSelected has it
    mov r14d, 1
walked:
    test edi, edi
    jnz mark
    mov byte ptr [rbx + 0x30], 0
    test r14d, r14d
    jnz done                           ; someone else is still marked: the squad stays
    xor edx, edx                       ; he was the last: the squad goes too
    jmp forward
mark:
    mov byte ptr [rbx + 0x30], dil
    mov edx, edi
forward:
    mov rcx, rsi
    mov rax, qword ptr [rsi]
    add rsp, 0x28
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    .byte 0x48, 0xff, 0x60, 0x50       ; rex.w jmp [rax+0x50], the squad's setSelected
own_only:
    mov byte ptr [rbx + 0x30], dil
done:
    add rsp, 0x28
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret

; SquadUnitSelectableFacet::isSelected, entered by a jmp over the stock
; function at logic.dll 0x4491c0. The stock code dereferences
; `this+0x28` as the squad's facet, but the selected-unit UI can still hold a
; soldier facet whose squad object is gone, and `[parent]` then reads a zero
; vtable and calls through it. A parent without a vtable is treated as no
; parent, so the soldier answers from his own enabled and mark bytes; a live
; parent must still report selected for him to be, as the patched getter did.
is_selected:                           ; rcx the facet; al the answer
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    mov rcx, qword ptr [rcx + 0x28]
    mov rdx, 0x800000000000            ; a user pointer is below this
    cmp rcx, 0x10000
    jb own                             ; +0x28 is not a pointer at all
    cmp rcx, rdx
    jae own
    cmp qword ptr [rcx], 0
    je own                             ; stale parent: follow his own mark instead
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x58]
    test al, al
    jz is_false
own:
    cmp byte ptr [rbx + 0x18], 0
    je is_false
    cmp byte ptr [rbx + 0x30], 0
    je is_false
    mov al, 1
    jmp is_done
is_false:
    xor al, al
is_done:
    add rsp, 0x20
    pop rbx
    ret
is_end:
