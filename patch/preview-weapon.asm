; Show the first matching weapon in squad previews.
;
; There are two preview builders that each leave the LAST weapon in their
; per-soldier descriptor:
;
;   fn_216b40  game.dll, the in-mission UnitInfo preview's
;              SquadPreviewDataProvider, which walks each soldier's gun list
;   fn_29a810  game.dll, the out-of-mission InfantryInfoPanel's descriptor
;              builder, which walks the squad's weapon vector (squad+0x270)
;              and, when a weapon carries no member index, writes it into
;              every descriptor
;
; logic.dll's preview builder (fn_1802029c0) then attaches the descriptor's
; weapons to the mannequins. Both entries below keep the first match in each
; slot. This does not track subsequent held-weapon changes in play. No
; simulation code changes.
;
; In fn_216b40 two stores fill the descriptor; both are guarded, and the two
; branch targets outside the hooked spans are restored through the descriptor's
; fixups (the scan's increment and the filter's fall-through). In fn_29a810 the
; one un-indexed store is guarded; its displaced add and the loop's condition
; are restored the same way.
;
; Live at the sites: rbp (the frame), r9 (the matched gun), rbx (the matched
; squad/holder), rsi/r14 (the scan cursor and its end), eax (the lookup result),
; and for fn_29a810 rdx (the weapon record) and rcx/r8 (the descriptor cursor
; and its end). r11 is a scratch register; the displaced code never reads it.

preview_secondary:
    cmp qword ptr [rbp - 0x71], 0
    jne preview_secondary_kept
    mov qword ptr [rbp - 0x71], r9
preview_secondary_kept:
    ; r13 survives across soldiers and supplies dead/missing-member previews
    ; at game+216f49/+216f82. Keep the first squad-wide fallback too.
    test r13, r13
    jne preview_loop
    mov r13, r9
    jmp preview_loop

preview_primary:
    test eax, eax
    jne preview_primary_store
    cmp byte ptr [rbx + 0xa9], al
    jne preview_loop
    mov r11, 0xaaaaaaaaaaaaaac2
    jmp r11
preview_primary_store:
    cmp qword ptr [rbp - 0x69], 0
    jne preview_loop
    mov qword ptr [rbp - 0x69], r9
preview_loop:
    mov r11, 0xaaaaaaaaaaaaaac1
    jmp r11

; The out-of-mission squad-management preview: one weapon record per squad
; entry, written into every descriptor when it has no member index. Guarded so
; the first record's weapon survives.
preview_squad:
    mov rax, qword ptr [rdx + 0xb8]
    cmp qword ptr [rcx + 0x28], 0
    jne preview_squad_kept
    mov qword ptr [rcx + 0x28], rax
preview_squad_kept:
    add rcx, 0x38
    mov r11, 0xaaaaaaaaaaaaaac3
    jmp r11
