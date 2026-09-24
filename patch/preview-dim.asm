; Dim the unselected soldiers of a partly selected squad in the 3D preview.
;
; game.dll's SquadPreviewDataProvider writes one 0x38-byte descriptor per
; soldier, whose +0x20 byte says dead (1) or alive (0). patch/preview-subset.asm
; (game.dll) writes 2 there for a living soldier who is not selected. logic.dll's
; preview builder (fn_2029c0) then reads the byte twice, and both reads are
; hooked here:
;
; - dim_pose   (0x203499) the pose: 2 is alive, so it reads as 0.
; - dim_mode   (0x2037f0) the silhouette: 1 keeps it; 2 dims the soldier, but
;              only when the squad also has a selected soldier (a 0 among the
;              builder's copy of the descriptors at [rbp+0x90], [rbp+0x98]) and
;              Core has written the material callback; anything else is left
;              with its own materials.
; - dim_part   (0x203830) the loop over the mannequin's renderers, which sets
;              each one's override material from the context's shared_ptr at
;              [ctx+0x20] (materials/dead.material). In dim mode the override is
;              the shared_ptr Core's callback returns for the part (its own
;              material's dimmed copy), and a part it has none for is skipped.
;
; The cell ({scratch}) holds the callback, `extern "C" fn(part, ctx) ->
; *const shared_ptr` (Core's preview module; 0 until it is written), at +0,
; and the mode for the soldier being built (0 none, 2 dim) at +8. The builder
; runs on the main thread, one soldier at a time.
;
; At both vtable-call sites rsp is 16-byte aligned, rax, rcx, rdx and r8 to r11
; are dead, and r15 is the context at dim_part.

dim_pose:
    movzx r14d, byte ptr [r12 + 0x20]  ; displaced: the descriptor's dead byte
    cmp r14d, 2
    jne dim_pose_done
    xor r14d, r14d
dim_pose_done:
    jmp 0x20349f

dim_mode:
    lea r11, [rip + {scratch}]
    mov byte ptr [r11 + 8], 0
    mov rax, qword ptr [rbp - 0x78]    ; the soldier's descriptor
    movzx eax, byte ptr [rax + 0x20]
    cmp eax, 1
    je dim_mode_parts                  ; dead: the stock silhouette
    cmp eax, 2
    jne dim_mode_skip
    cmp qword ptr [r11], 0
    je dim_mode_skip
    mov rcx, qword ptr [rbp + 0x90]
    mov rdx, qword ptr [rbp + 0x98]
dim_mode_scan:
    cmp rcx, rdx
    jae dim_mode_skip                  ; nobody selected: nothing to set apart
    movzx eax, byte ptr [rcx + 0x20]
    add rcx, 0x38
    test eax, eax
    jnz dim_mode_scan
    mov byte ptr [r11 + 8], 2
dim_mode_parts:
    jmp 0x2037fe
dim_mode_skip:
    jmp 0x203897

dim_part:
    lea r11, [rip + {scratch}]
    cmp byte ptr [r11 + 8], 2
    jne dim_part_stock
    mov rax, qword ptr [r11]
    mov rcx, qword ptr [rsi]           ; the renderer
    mov rdx, r15                       ; the preview context
    sub rsp, 0x20
    call rax
    add rsp, 0x20
    test rax, rax
    jz dim_part_next
    mov rdx, rax
    mov rcx, qword ptr [rsi]
    mov rax, qword ptr [rcx]
    jmp 0x20383a                       ; call [rax+0x58], the override setter
dim_part_stock:
    mov rcx, qword ptr [rsi]           ; displaced
    mov rax, qword ptr [rcx]
    lea rdx, [r15 + 0x20]
    jmp 0x20383a
dim_part_next:
    jmp 0x20383d
