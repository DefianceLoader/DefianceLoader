//! x86-64 instruction length, so a hook can displace whole instructions
//! without the caller stating a byte count and risking a cut instruction.
//!
//! This is deliberately conservative: it decodes the integer, x87 and SSE
//! encodings that real code uses, and returns an error for anything it is not
//! sure of (VEX/EVEX, 3DNow!, a few invalid bytes). A hook on such a site is
//! refused rather than installed with a guessed length. A wrong length is not
//! a wrong result; it is a corrupted process, so refusing is the only safe
//! failure.
//!
//! The assembled patches do not use this: their descriptors carry exact
//! displaced bytes, verified by `--self-test`. This exists for `Api::hook`,
//! where a plugin has only an address.

/// The length of the instruction at the start of `code`.
pub fn instruction_length(code: &[u8]) -> Result<usize, String> {
    decode(code).map(|(length, _)| length)
}

/// Validate an exact span for a trampoline that copies instructions verbatim.
/// Relative control flow and instruction-pointer-relative memory operands need
/// relocation, which this engine does not implement. Refuse before any write.
pub fn validate_copy(code: &[u8]) -> Result<(), String> {
    let mut offset = 0;
    while offset < code.len() {
        let (length, relative) = decode(&code[offset..])?;
        if relative {
            return Err(format!("instruction at +{offset:#x} requires relocation"));
        }
        offset += length;
    }
    Ok(())
}

fn decode(code: &[u8]) -> Result<(usize, bool), String> {
    let mut at = 0usize;
    let mut opsize = false;
    let mut addrsize = false;
    let mut rex = 0u8;

    // prefixes
    loop {
        let byte = *code.get(at).ok_or("the instruction runs off the end")?;
        match byte {
            0x66 => {
                opsize = true;
                at += 1;
            }
            0x67 => {
                addrsize = true;
                at += 1;
            }
            // segment overrides and lock/rep, which do not change the length
            0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 | 0xF0 | 0xF2 | 0xF3 => at += 1,
            // REX must sit last, but tolerate repeats
            0x40..=0x4F => {
                rex = byte;
                at += 1;
            }
            _ => break,
        }
    }

    let opcode = *code.get(at).ok_or("the instruction runs off the end")?;
    at += 1;

    let mut relative = matches!(opcode, 0x70..=0x7F | 0xE0..=0xE3 | 0xE8 | 0xE9 | 0xEB);

    // Which map, and the opcode's form in it.
    let form = if opcode == 0x0F {
        let second = *code.get(at).ok_or("the instruction runs off the end")?;
        at += 1;
        relative |= matches!(second, 0x80..=0x8F);
        match second {
            0x38 => {
                at += 1; // the third opcode byte, all of the map take ModRM
                Some(Form {
                    modrm: true,
                    imm: Imm::None,
                })
            }
            0x3A => {
                at += 1; // all of the map take ModRM and an imm8
                Some(Form {
                    modrm: true,
                    imm: Imm::B1,
                })
            }
            _ => two_byte(second),
        }
    } else {
        one_byte(opcode)
    }
    .ok_or_else(|| format!("unsupported opcode {opcode:#04x} for a length"))?;

    let mut reg = 0usize;
    if form.modrm {
        let modrm = *code.get(at).ok_or("the instruction runs off the end")?;
        at += 1;
        reg = ((modrm >> 3) & 7) as usize;
        let mode = modrm >> 6;
        // mod=00 r/m=101 is RIP-relative (EIP-relative with 67). A SIB
        // with base=101 is absolute disp32 and must not be mistaken for it.
        relative |= mode == 0 && modrm & 7 == 5;
        // XBEGIN has a relative immediate, unlike the other C7 encodings.
        relative |= opcode == 0xC7 && modrm == 0xF8;
        if mode != 3 {
            let mut base = modrm & 7;
            if base == 4 {
                let sib = *code.get(at).ok_or("the instruction runs off the end")?;
                at += 1;
                base = sib & 7;
            }
            let displacement = match mode {
                0 => {
                    if base == 5 {
                        4
                    } else {
                        0
                    }
                }
                1 => 1,
                _ => 4,
            };
            at += displacement;
        }
    }

    // F6/F7 are the only reg-dependent immediates among the common groups:
    // only /0 and /1 (test) carry one. For a two-byte opcode `opcode` is 0x0F,
    // so this only matches the one-byte map.
    let mut imm = form.imm;
    if opcode == 0xF6 {
        imm = if reg <= 1 { Imm::B1 } else { Imm::None };
    } else if opcode == 0xF7 {
        imm = if reg <= 1 { Imm::Z } else { Imm::None };
    }

    at += imm_size(imm, opsize, rex & 0x08 != 0, addrsize);
    if at > code.len() {
        return Err("the instruction runs off the end".to_string());
    }
    if at > 15 {
        return Err("instruction exceeds the x86-64 limit of 15 bytes".to_string());
    }
    Ok((at, relative))
}

/// How many whole instructions cover at least `minimum` bytes of `code`.
pub fn displaced(code: &[u8], minimum: usize) -> Result<usize, String> {
    let mut total = 0usize;
    while total < minimum {
        if total >= code.len() {
            return Err(format!(
                "only {total} bytes are available, and {minimum} are needed"
            ));
        }
        total += instruction_length(&code[total..])?;
    }
    Ok(total)
}

#[derive(Clone, Copy)]
enum Imm {
    None,
    B1,
    B2,
    B3,
    B4,
    /// operand size: imm16 with 66, else imm32
    Z,
    /// imm64 with REX.W, imm16 with 66, else imm32
    V,
    /// an moffs, whose size follows the address size
    Moffs,
}

#[derive(Clone, Copy)]
struct Form {
    modrm: bool,
    imm: Imm,
}

fn imm_size(imm: Imm, opsize: bool, rex_w: bool, addrsize: bool) -> usize {
    match imm {
        Imm::None => 0,
        Imm::B1 => 1,
        Imm::B2 => 2,
        Imm::B3 => 3,
        Imm::B4 => 4,
        Imm::Z => {
            if opsize {
                2
            } else {
                4
            }
        }
        Imm::V => {
            if rex_w {
                8
            } else if opsize {
                2
            } else {
                4
            }
        }
        Imm::Moffs => {
            if addrsize {
                4
            } else {
                8
            }
        }
    }
}

/// The one-byte opcode map. `None` is an opcode this does not decode.
fn one_byte(op: u8) -> Option<Form> {
    use Imm::*;
    let f = |modrm, imm| Some(Form { modrm, imm });
    match op {
        // arithmetic and logic groups: /r forms, then imm8, then immz
        0x00 | 0x01 | 0x02 | 0x03 | 0x08 | 0x09 | 0x0A | 0x0B | 0x10 | 0x11 | 0x12 | 0x13
        | 0x18 | 0x19 | 0x1A | 0x1B | 0x20 | 0x21 | 0x22 | 0x23 | 0x28 | 0x29 | 0x2A | 0x2B
        | 0x30 | 0x31 | 0x32 | 0x33 | 0x38 | 0x39 | 0x3A | 0x3B => f(true, None),
        0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => f(false, B1),
        0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => f(false, Z),
        0x50..=0x5F => f(false, None), // push/pop r64
        0x63 => f(true, None),         // movsxd
        0x68 => f(false, Z),           // push immz
        0x69 => f(true, Z),            // imul r, r/m, immz
        0x6A => f(false, B1),          // push imm8
        0x6B => f(true, B1),           // imul r, r/m, imm8
        0x6C..=0x6F => f(false, None), // ins/outs
        0x70..=0x7F => f(false, B1),   // jcc rel8
        0x80 => f(true, B1),
        0x81 => f(true, Z),
        0x83 => f(true, B1),
        0x84..=0x8F => f(true, None),
        0x90..=0x99 | 0x9B..=0x9F => f(false, None),
        0xA0..=0xA3 => f(false, Moffs),
        0xA4..=0xA7 => f(false, None),
        0xA8 => f(false, B1),
        0xA9 => f(false, Z),
        0xAA..=0xAF => f(false, None),
        0xB0..=0xB7 => f(false, B1), // mov r8, imm8
        0xB8..=0xBF => f(false, V),  // mov r, imm(v)
        0xC0 | 0xC1 => f(true, B1),
        0xC2 => f(false, B2), // ret imm16
        0xC3 => f(false, None),
        0xC6 => f(true, B1),
        0xC7 => f(true, Z),
        0xC8 => f(false, B3), // enter imm16, imm8
        0xC9 => f(false, None),
        0xCA => f(false, B2), // retf imm16
        0xCB | 0xCC | 0xCF => f(false, None),
        0xCD => f(false, B1), // int imm8
        0xD0..=0xD3 => f(true, None),
        0xD7 => f(false, None),
        0xD8..=0xDF => f(true, None), // x87
        0xE0..=0xE3 => f(false, B1),  // loop/jcxz rel8
        0xE4..=0xE7 => f(false, B1),  // in/out imm8
        0xE8 | 0xE9 => f(false, B4),  // call/jmp rel32
        0xEB => f(false, B1),         // jmp rel8
        0xEC..=0xEF => f(false, None),
        0xF1 | 0xF4 | 0xF5 => f(false, None),
        0xF6 => f(true, None), // group 3, adjusted by reg
        0xF7 => f(true, None),
        0xF8..=0xFD => f(false, None),
        0xFE | 0xFF => f(true, None), // group 4/5
        _ => Option::None,
    }
}

/// The two-byte (0F) opcode map.
fn two_byte(op: u8) -> Option<Form> {
    use Imm::*;
    let f = |modrm, imm| Some(Form { modrm, imm });
    match op {
        0x00 | 0x01 => f(true, None), // group 6/7
        0x02 | 0x03 => f(true, None), // lar/lsl
        // these read no operands from the instruction stream
        0x05..=0x09 | 0x0B | 0x0E | 0x30..=0x37 | 0x77 | 0xA2 | 0xAA | 0xB9 => f(false, None),
        0x0D => f(true, None),
        0x10..=0x17 | 0x18..=0x1F | 0x20..=0x23 | 0x28..=0x2F => f(true, None),
        0x40..=0x4F | 0x50..=0x5F | 0x60..=0x6F => f(true, None),
        0x70 | 0x71 | 0x72 | 0x73 => f(true, B1),
        0x74..=0x76 | 0x78 | 0x79 | 0x7C..=0x7F => f(true, None),
        0x80..=0x8F => f(false, B4),                 // jcc rel32
        0x90..=0x9F => f(true, None),                // setcc
        0xA0 | 0xA1 | 0xA8 | 0xA9 => f(false, None), // push/pop fs/gs
        0xA3 | 0xA5 | 0xAB | 0xAD | 0xAE | 0xAF => f(true, None),
        0xA4 | 0xAC => f(true, B1), // shld/shrd imm8
        0xB0 | 0xB1 => f(true, None),
        0xB2..=0xB8 => f(true, None),
        0xBA => f(true, B1), // group 8
        0xBB..=0xBF => f(true, None),
        0xC0 | 0xC1 | 0xC3 | 0xC7 => f(true, None),
        0xC2 | 0xC4 | 0xC5 | 0xC6 => f(true, B1),
        0xC8..=0xCF => f(false, None), // bswap
        0xD0..=0xFF => f(true, None),
        _ => Option::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn lengths_of_individual_instructions() {
        for (text, want) in [
            ("e813290000", 5),            // call rel32
            ("488b7c2430", 5),            // mov rdi, [rsp+0x30]
            ("4053", 2),                  // rex + push rbx
            ("4881ecc0000000", 7),        // sub rsp, imm32
            ("4883ec28", 4),              // sub rsp, imm8
            ("488b02", 3),                // mov rax, [rdx]
            ("488b4c2408", 5),            // mov rcx, [rsp+8] (SIB)
            ("e9a85dffff", 5),            // jmp rel32
            ("ebfe", 2),                  // jmp -2
            ("c3", 1),                    // ret
            ("cc", 1),                    // int3
            ("48b80000000000000000", 10), // mov rax, imm64
            ("b901000000", 5),            // mov ecx, imm32
            ("83c001", 3),                // add eax, 1
        ] {
            assert_eq!(instruction_length(&unhex(text)).unwrap(), want, "{text}");
        }
    }

    #[test]
    fn real_sequences_sum_to_their_displaced_length() {
        // taken from out/payload.json: the bytes the assembled patches displace
        for text in [
            "e813290000",
            "488b7c2430",
            "40534881ecc0000000",
            "48895c2408",
            "4883ec28488b02",
            "488bc1488b49284885c97407488b0148ff6050885030c3cccccccc",
            "e80266deff",
            "e9a85dffffcc",
        ] {
            let bytes = unhex(text);
            assert_eq!(
                displaced(&bytes, bytes.len()).unwrap(),
                bytes.len(),
                "{text}"
            );
        }
    }

    #[test]
    fn unsupported_encodings_are_refused() {
        // VEX (two- and three-byte), 3DNow!, and the invalid ESC byte
        assert!(instruction_length(&unhex("c5f877")).is_err());
        assert!(instruction_length(&unhex("c4e27d18")).is_err());
        assert!(instruction_length(&unhex("0f0fc0")).is_err());
        assert!(instruction_length(&unhex("d6")).is_err());
    }

    #[test]
    fn copied_spans_refuse_relative_instructions() {
        for hex in [
            "e800000000",
            "e900000000",
            "eb00",
            "7500",
            "0f8500000000",
            "e200",
            "e300",
            "488b0500000000",
            "488d0500000000",
            "ff2500000000",
            "678b0500000000",
            "c7f800000000",
            "90e800000000", // check every instruction, not only the first
        ] {
            assert!(
                validate_copy(&unhex(hex))
                    .unwrap_err()
                    .contains("relocation"),
                "{hex}"
            );
        }
    }

    #[test]
    fn copied_spans_accept_position_independent_operands() {
        for hex in [
            "40534883ec28",
            "488b442408",
            "488b042500100000",
            "b801000000c3",
        ] {
            validate_copy(&unhex(hex)).unwrap();
        }
    }

    #[test]
    fn copied_spans_reject_partial_and_overlong_instructions() {
        assert!(validate_copy(&unhex("b8010000")).is_err());
        let mut overlong = vec![0x66; 15];
        overlong.push(0x90);
        assert!(instruction_length(&overlong).is_err());
    }
}
