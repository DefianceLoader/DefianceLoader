; Diagnostic: who reads a squad's behaviour (G). Injector only.
;
; Behaviour is a dword on the squad's AI, SquadAiFacet +0x200, read through
; vt+0x88 (fn_436f30, mov eax, [rcx+0x200]; ret); a soldier's own getter
; answers 0. Static search found only some of its readers, so this replaces
; the getter with one that counts its callers: a table of 32 entries, each the
; caller's return address and how often it asked, which --select-probe prints.
; A full table stops counting new callers; concurrent callers can race an
; entry, which costs a count, not correctness. The answer is the stock one.
;
; {cursor} is the table.

census_behaviour:                      ; rcx the squad's AI, as fn_436f30
    mov r10, qword ptr [rsp]           ; who asked
    lea r11, [rip + {cursor}]
    mov eax, 32
census_scan:
    cmp qword ptr [r11], r10
    je census_hit
    cmp qword ptr [r11], 0
    je census_new
    add r11, 16
    dec eax
    jnz census_scan
    jmp census_answer
census_new:
    mov qword ptr [r11], r10
census_hit:
    inc qword ptr [r11 + 8]
census_answer:
    mov eax, dword ptr [rcx + 0x200]
    ret
