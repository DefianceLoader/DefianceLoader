//! Reading the signature text this project already writes.
//!
//! `tools/sigs.py` encodes a signature as hex, two digits per byte, with `??`
//! for every wildcarded byte (`488b05????????4885c0`). A plugin author may find
//! spaces easier to read, so both are accepted: whitespace is stripped first,
//! and `??`, `?`, and `..` all mean the same wildcard.

/// A parsed signature: `None` where a byte is wildcarded.
pub fn parse(text: &str) -> Result<Vec<Option<u8>>, String> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let compact = compact.replace("..", "??");
    let mut out = Vec::with_capacity(compact.len() / 2);
    let mut chars = compact.chars().peekable();
    while let Some(first) = chars.next() {
        let second = chars
            .next()
            .ok_or_else(|| format!("`{text}` has an odd number of digits"))?;
        match (first, second) {
            ('?', '?') => out.push(None),
            _ => {
                let pair = [first, second].iter().collect::<String>();
                let byte = u8::from_str_radix(&pair, 16)
                    .map_err(|_| format!("`{pair}` in `{text}` is not a byte or `??`"))?;
                out.push(Some(byte));
            }
        }
    }
    if out.is_empty() {
        return Err(format!("`{text}` is empty"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn continuous_and_spaced_agree() {
        assert_eq!(parse("488b??c0").unwrap(), parse("48 8B ?? c0").unwrap());
        assert_eq!(
            parse("488b??c0").unwrap(),
            vec![Some(0x48), Some(0x8b), None, Some(0xc0)]
        );
    }

    #[test]
    fn rejects_junk() {
        assert!(parse("488").is_err());
        assert!(parse("48zz").is_err());
        assert!(parse("").is_err());
    }
}
