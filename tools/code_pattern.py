"""Masked byte patterns that find a function again in another build."""
import re

from capstone.x86 import X86_OP_IMM, X86_OP_MEM, X86_REG_RIP


def pattern(image, rva, size=256):
    """A regex matching `size` bytes of code at `rva`, with every RIP-relative
    displacement and relative branch target masked: those move between builds
    while the instructions around them do not."""
    raw = bytearray(image.read(rva, size))
    mask = bytearray(b"\1" * len(raw))
    for ins in image.md.disasm(raw, rva):
        relative = ins.group(1) or ins.group(2)  # jump/call
        for op in ins.operands:
            if op.type == X86_OP_MEM and op.mem.base == X86_REG_RIP:
                start = ins.address - rva + ins.disp_offset
                mask[start:start + ins.disp_size] = b"\0" * ins.disp_size
            elif op.type == X86_OP_IMM and relative:
                start = ins.address - rva + ins.imm_offset
                mask[start:start + ins.imm_size] = b"\0" * ins.imm_size
    return b"".join(re.escape(bytes([v])) if m else b"." for v, m in zip(raw, mask))
