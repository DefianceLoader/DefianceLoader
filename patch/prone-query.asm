; "Is it prone?" for the soldiers the player picked, not only for the squad.
;
; AiUtilsImpl vt+0x2c0 (logic.dll fn_110bb0) answers whether an entity is prone
; from its squad's flag, SquadAiFacet +0x29e. The squad panel (game.dll
; fn_23ec60, 0x23f3c0) asks it for every selected entity and offers V as stand
; up only when all of them are prone. A squad is what gets asked, so with one
; soldier pinned prone in a standing squad, V only ever offered lie down.
;
; For a squad this answers for the soldiers an order would reach: the marked
; ones while the marks discriminate (some but not all selectable soldiers
; marked, the rule the move filter and the pose split use), otherwise all of
; them. It says prone only if every one of those is, where a soldier is his pin
; if he has one (patch/posture-gate.asm) and his squad's flag otherwise, so a
; squad whose soldiers were all laid down one by one offers stand up. Anything
; that is not a squad with soldiers gets the stock answer, which is the
; original function run through stock_prone: its displaced prologue, then a
; jump back into it.
;
; Entered by a jmp over fn_110bb0's push rbx and sub rsp, 0x20, as that
; function: rcx AiUtilsImpl (unused by it), rdx the entity, the answer in al.
; The prologue is the shape tools/build.py describes in its unwind record.

prone_query:
    push rbx
    push rbp
    push rsi
    push rdi
    push r12
    push r13
    push r14
    push r15
    sub rsp, 0x28
    mov r14, rcx                       ; this, for the stock answer
    mov rbx, rdx                       ; the entity asked about
    mov rdx, rbx
    call stock_prone
    movzx r15d, al                     ; the stock answer, the squad's flag
    test rbx, rbx
    jz answer
    mov rax, qword ptr [rbx]
    mov edx, 0x10
    mov rcx, rbx
    call qword ptr [rax + 0x98]        ; a squad?
    test al, al
    jz answer

    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0xb0]        ; its facets
    test rax, rax
    jz answer
    mov rcx, qword ptr [rax + 0x28]
    test rcx, rcx
    jz answer
    mov rax, qword ptr [rcx]
    call qword ptr [rax + {squad_roster}]
    test rax, rax
    jz answer
    mov rdx, qword ptr [rax]
    mov rcx, rax
    call qword ptr [rdx + 0x68]        ; the member vector
    mov r12, qword ptr [rax]
    mov r13, qword ptr [rax + 8]

    xor esi, esi                       ; marked
    xor edi, edi                       ; selectable
    mov word ptr [rsp + 0x20], 0       ; +0x20 a marked soldier upright, +0x21 any
    mov rbp, r12
next:
    cmp rbp, r13
    jae counted
    mov rcx, qword ptr [rbp]
    add rbp, 8
    test rcx, rcx
    jz next
    mov rax, qword ptr [rcx]
    call qword ptr [rax + 0xb0]
    test rax, rax
    jz next
    mov rbx, qword ptr [rax + 0x50]    ; his selectable facet (the entity is done with)
    test rbx, rbx
    jz next
    cmp byte ptr [rbx + 0x18], 0
    je next                            ; not selectable: on neither side of the rule
    inc edi
    mov r14d, r15d                     ; unpinned, he is what his squad is
    cmp word ptr [rbx + 0x32], 0x7a5e
    jne judged
    xor r14d, r14d
    cmp byte ptr [rbx + 0x31], 3
    sete r14b                          ; pinned, he is his pin
judged:
    test r14d, r14d
    jnz ask
    mov byte ptr [rsp + 0x21], 1       ; someone in the squad is upright
ask:
    mov rax, qword ptr [rbx]
    mov rcx, rbx
    call qword ptr [rax + 0x58]
    test al, al
    jz next
    inc esi
    test r14d, r14d
    jnz next
    mov byte ptr [rsp + 0x20], 1       ; a marked soldier is upright
    jmp next
counted:
    ; the soldiers an order would reach: the marked ones while the marks
    ; discriminate, otherwise every selectable soldier
    test edi, edi
    jz answer                          ; no soldiers to judge: the stock answer
    movzx eax, byte ptr [rsp + 0x21]
    test esi, esi
    jz decide                          ; nobody marked
    cmp esi, edi
    jae decide                         ; everybody marked
    movzx eax, byte ptr [rsp + 0x20]
decide:
    xor eax, 1                         ; prone only if none of them is upright
    mov r15d, eax
answer:
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

; the original fn_110bb0, callable: its displaced prologue, then back into it
stock_prone:
    push rbx
    sub rsp, 0x20
    jmp 0x110bb6
