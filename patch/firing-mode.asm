; Firing mode (T) for the marked soldiers only.
;
; Whether a moving unit stops to fire stationary weapons is one byte on the
; squad's AI, SquadAiFacet +0x228: vt+0x380 reads it, vt+0x388 writes it, and a
; soldier's own setter is `ret 0`. The panel's T (game.dll 0x186570) sets every
; selected squad through vt+0x388. Soldiers read it in three places, all found:
;
;   HumanAiFacet vt+0x380 (fn_2ae8f0)  false in a vehicle, else a tail call to
;                                      his squad's vt+0x380 (at 0x2ae9ac)
;   fn_2caeb0, at 0x2caf8b             the same, inlined: rsi his AI, a direct
;                                      call to the squad's vt+0x380
;   fn_fb0a0 (attack-move and others)  asks the soldier's own AI, so the first
;
; A soldier gets a firing pin in padding of his SquadUnitSelectableFacet, next
; to the enabled byte: +0x1b the pin (0 none, else the value plus one), +0x1a
; kept for behaviour, and +0x1c a 16-bit marker, 0x7a5f, that makes both valid.
; The constructor never initialises those bytes, so they are zeroed when the
; marker is first written; anything without it reads as no pin.
;
; - firing_set replaces the squad's setter. While the marks discriminate (some
;   but not all selectable soldiers marked, the rule the move filter uses) the
;   marked soldiers are pinned to the value and the squad keeps its own;
;   otherwise the squad's pins are cleared and its byte is written as before.
; - firing_soldier and firing_call answer with a soldier's pin before his
;   squad's byte, which squad_flag reads directly, so they never reach the
;   squad's getter.
; - firing_ui replaces that getter, which is left to the panel: it answers for
;   the soldiers an order would reach, true if any of them stops to fire,
;   which is the test T itself makes before flipping the mode.
;
; The routines with frames have the prologue shapes tools/build.py describes in
; their unwind records.

firing_set:                            ; rcx the squad's AI, dl the value
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x28
    mov qword ptr [rsp + 0x20], rcx    ; the squad's AI, for its own byte
    movzx r15d, dl
    call list_members                  ; r12, r13 the members, r14 the squad's facet
    test eax, eax
    jz set_squad
    xor esi, esi                       ; marked
    xor edi, edi                       ; selectable
    mov rbp, r12
set_count:
    cmp rbp, r13
    jae set_counted
    call next_soldier                  ; rbx his facet, or zero
    test rbx, rbx
    jz set_count
    inc edi
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0x58]
    test al, al
    jz set_count
    inc esi
    jmp set_count
set_counted:
    test esi, esi
    jz unpin_all                       ; nobody marked
    cmp esi, edi
    jae unpin_all                      ; everybody marked
    mov rbp, r12
pin_next:
    cmp rbp, r13
    jae set_done
    call next_soldier
    test rbx, rbx
    jz pin_next
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0x58]
    test al, al
    jz pin_next
    cmp word ptr [rbx + 0x1c], 0x7a5f
    je pin_marked
    mov word ptr [rbx + 0x1a], 0       ; his first pin: nothing else set yet
    mov word ptr [rbx + 0x1c], 0x7a5f
pin_marked:
    lea eax, [r15 + 1]
    mov byte ptr [rbx + 0x1b], al
    jmp pin_next
unpin_all:
    mov rbp, r12
unpin_next:
    cmp rbp, r13
    jae set_squad
    call next_soldier
    test rbx, rbx
    jz unpin_next
    cmp word ptr [rbx + 0x1c], 0x7a5f
    jne unpin_next
    mov byte ptr [rbx + 0x1b], 0
    jmp unpin_next
set_squad:
    mov rax, qword ptr [rsp + 0x20]
    mov byte ptr [rax + 0x228], r15b
set_done:
    add rsp, 0x28
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret

firing_ui:                             ; rcx the squad's AI; al the answer
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x28
    movzx r15d, byte ptr [rcx + 0x228] ; the squad's own byte
    call list_members
    test eax, eax
    jz ui_answer
    xor esi, esi                       ; marked
    xor edi, edi                       ; selectable
    mov word ptr [rsp + 0x20], 0       ; +0x20 a marked soldier stops, +0x21 any
    mov rbp, r12
ui_next:
    cmp rbp, r13
    jae ui_counted
    call next_soldier
    test rbx, rbx
    jz ui_next
    inc edi
    mov eax, r15d                      ; unpinned, he is what his squad is
    cmp word ptr [rbx + 0x1c], 0x7a5f
    jne ui_judged
    movzx ecx, byte ptr [rbx + 0x1b]
    test ecx, ecx
    jz ui_judged
    lea eax, [rcx - 1]
ui_judged:
    mov byte ptr [rsp + 0x22], al
    or byte ptr [rsp + 0x21], al
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0x58]
    test al, al
    jz ui_next
    inc esi
    movzx eax, byte ptr [rsp + 0x22]
    or byte ptr [rsp + 0x20], al
    jmp ui_next
ui_counted:
    test edi, edi
    jz ui_answer                       ; no soldiers: the squad's byte
    movzx r15d, byte ptr [rsp + 0x21]
    test esi, esi
    jz ui_answer
    cmp esi, edi
    jae ui_answer
    movzx r15d, byte ptr [rsp + 0x20]
ui_answer:
    mov eax, r15d
    add rsp, 0x28
    pop r15
    pop r14
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    ret

; The members of the squad whose AI is rcx: r12 and r13 bound them and r14 is
; the squad's own facet; eax is 0 when they cannot be listed. Uses rbx, which
; both callers have saved.
list_members:
    sub rsp, 0x28
    xor eax, eax
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz listed
    mov rbx, qword ptr [rcx + 0x10]    ; the squad entity
    test rbx, rbx
    jz listed
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz listed
    mov r14, qword ptr [rax + 0x50]    ; the squad's own facet
    mov rcx, qword ptr [rax + 0x28]
    xor eax, eax
    test rcx, rcx
    jz listed
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz listed
    mov rdx, qword ptr [rax]
    mov rcx, rax
    call qword ptr [rdx + 0x68]
    mov r12, qword ptr [rax]
    mov r13, qword ptr [rax + 8]
    mov eax, 1
listed:
    add rsp, 0x28
    ret
list_end:

; rbx = the facet of the soldier at [rbp], rbp advanced; zero when he has none,
; is not selectable, or is not a soldier of the squad whose facet is r14
next_soldier:
    sub rsp, 0x28
    xor ebx, ebx
    mov rcx, qword ptr [rbp]
    add rbp, 8
    test rcx, rcx
    jz next_done
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz next_done
    mov rcx, qword ptr [rax + 0x50]
    test rcx, rcx
    jz next_done
    cmp byte ptr [rcx + 0x18], 0
    je next_done                       ; not selectable
    cmp qword ptr [rcx + 0x28], r14
    jne next_done                      ; not this squad's soldier
    mov rbx, rcx
next_done:
    add rsp, 0x28
    ret
next_end:

; fn_2ae8f0, HumanAiFacet vt+0x380, entered by a jmp over its first
; instruction: rcx his AI. A pinned soldier answers with his pin, except in a
; vehicle, where the stock answer is false; anyone else goes the stock way.
firing_soldier:
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    call fire_pin
    test eax, eax
    jz soldier_stock
    mov dword ptr [rsp + 0x30], eax    ; the pin, in the caller's home space
    mov rcx, qword ptr [rbx + 0x1f0]
    test rcx, rcx
    jz soldier_pinned
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz soldier_pinned
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0x88]        ; in a vehicle?
    test al, al
    jz soldier_pinned
    xor eax, eax
    jmp soldier_done
soldier_pinned:
    mov eax, dword ptr [rsp + 0x30]
    dec eax
soldier_done:
    add rsp, 0x20
    pop rbx
    ret
soldier_stock:
    mov rcx, rbx
    add rsp, 0x20
    pop rbx
soldier_resume:
    mov qword ptr [rsp + 8], rbx       ; fn_2ae8f0's first instruction, displaced
    jmp 0x2ae8f5

; fn_2ae8f0's tail call to the squad's getter, and the stock answer: the byte
squad_flag:                            ; rcx the squad's AI
    movzx eax, byte ptr [rcx + 0x228]
    ret

; fn_2caeb0's call to the squad's getter at 0x2caf8b: rcx the squad's AI, rsi
; the soldier's AI
firing_call:
    push rbx
    sub rsp, 0x20
    mov rbx, rcx
    mov rcx, rsi
    call fire_pin
    test eax, eax
    jz call_squad
    dec eax
    jmp call_done
call_squad:
    movzx eax, byte ptr [rbx + 0x228]
call_done:
    add rsp, 0x20
    pop rbx
    ret
call_end:

; eax = the firing pin of the soldier whose AI object is rcx, 0 when none
fire_pin:
    sub rsp, 0x28
    xor eax, eax
    mov rcx, qword ptr [rcx + 0x10]
    test rcx, rcx
    jz fire_none
    mov rcx, qword ptr [rcx + 0x10]    ; his entity
    test rcx, rcx
    jz fire_none
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz fire_none
    mov rcx, qword ptr [rax + 0x50]
    xor eax, eax
    test rcx, rcx
    jz fire_none
    cmp word ptr [rcx + 0x1c], 0x7a5f
    jne fire_none
    movzx eax, byte ptr [rcx + 0x1b]
fire_none:
    add rsp, 0x28
    ret
fire_end:
