//! Finding a masked signature in a module, exactly as `tools/sigs.py` does,
//! and turning the found sites into a per-site distance.
//!
//! A signature must match once. `Moves` also enforces the project's rule that
//! every site sharing a function moved by the same distance, which is what
//! lets an address that lies between two sites be moved with either of them.
//! The loader resolves a plugin's `find_pattern` with the same `scan` below, so
//! a pattern unique in the file a build was written for does not silently
//! become ambiguous in memory.

use super::pattern::parse;

/// One site's signature: its bytes, `None` where a field encodes a distance
/// to other code, the window's rva in the build the patch was written for,
/// and the function it lies in, whose sites must all move together.
#[derive(Clone, Debug)]
pub struct Site {
    pub name: String,
    pub start: usize,
    pub group: usize,
    pub pattern: Vec<Option<u8>>,
}

impl Site {
    /// A site whose pattern came from `parse`.
    pub fn new(
        name: impl Into<String>,
        start: usize,
        group: usize,
        pattern: Vec<Option<u8>>,
    ) -> Self {
        Self {
            name: name.into(),
            start,
            group,
            pattern,
        }
    }

    /// A site named after itself, in a group of its own. What a plugin's
    /// `find_pattern` needs: it cares only whether the match is unique.
    pub fn lone(name: impl Into<String>, pattern: Vec<Option<u8>>) -> Self {
        Self::new(name, 0, 0, pattern)
    }

    pub fn from_text(name: impl Into<String>, rva: usize, text: &str) -> Result<Self, String> {
        Ok(Self::new(name, rva, 0, parse(text)?))
    }
}

/// Every offset in `image` where `pattern` occurs. The first fixed byte is
/// skipped to, one candidate at a time, so wildcards at the front cost nothing.
pub fn scan(image: &[u8], pattern: &[Option<u8>]) -> Vec<usize> {
    let mut out = Vec::new();
    let Some((lead, first)) = pattern
        .iter()
        .enumerate()
        .find_map(|(i, b)| b.map(|b| (i, b)))
    else {
        return out;
    };
    if image.len() < pattern.len() {
        return out;
    }
    let last = image.len() - pattern.len();
    let mut at = 0;
    while at <= last {
        let Some(skip) = image[at + lead..=last + lead]
            .iter()
            .position(|&b| b == first)
        else {
            break;
        };
        let start = at + skip;
        let window = &image[start..start + pattern.len()];
        if pattern
            .iter()
            .zip(window)
            .all(|(p, &b)| p.map_or(true, |p| p == b))
        {
            out.push(start);
        }
        at = start + 1;
    }
    out
}

/// Where the sites are in this build, as a distance per site.
#[derive(Debug)]
pub struct Moves {
    sites: Vec<Site>,
    deltas: Vec<isize>,
}

impl Moves {
    pub fn locate(sites: &[Site], image: &[u8]) -> Result<Self, String> {
        let mut deltas = Vec::new();
        for site in sites {
            match scan(image, &site.pattern).as_slice() {
                [one] => deltas.push(*one as isize - site.start as isize),
                [] => return Err(format!("the {} site is not in this build", site.name)),
                many => {
                    return Err(format!(
                        "the {} site's signature matches {} places in this build",
                        site.name,
                        many.len()
                    ))
                }
            }
        }
        for (i, a) in sites.iter().enumerate() {
            for (j, b) in sites.iter().enumerate().skip(i + 1) {
                if a.group == b.group && deltas[i] != deltas[j] {
                    return Err(format!(
                        "the {} and {} sites share a function but have moved apart",
                        a.name, b.name
                    ));
                }
            }
        }
        Ok(Self {
            sites: sites.to_vec(),
            deltas,
        })
    }

    /// `rva` from the build the patch was written for, in this one: moved with
    /// the site whose signature holds it, its window's end included, since a
    /// resume point may be the address just past it.
    pub fn at(&self, rva: usize) -> Result<usize, String> {
        let mut delta = None;
        for (site, &d) in self.sites.iter().zip(&self.deltas) {
            if rva >= site.start && rva <= site.start + site.pattern.len() {
                if delta.is_some_and(|seen| seen != d) {
                    return Err(format!("{rva:#x} lies in two sites that moved differently"));
                }
                delta = Some(d);
            }
        }
        delta
            .map(|d| (rva as isize + d) as usize)
            .ok_or_else(|| format!("{rva:#x} is in no site's signature"))
    }

    pub fn report(&self) -> String {
        let moved = self.deltas.iter().filter(|&&d| d != 0).count();
        let mut spread: Vec<isize> = self.deltas.clone();
        spread.sort();
        spread.dedup();
        format!(
            "{} sites found by signature, {moved} of them moved ({})",
            self.sites.len(),
            spread
                .iter()
                .map(|d| format!("{d:+#x}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::parse;

    #[test]
    fn finds_only_where_the_fixed_bytes_are() {
        let image = [0x00, 0x48, 0x8b, 0x11, 0x48, 0x8b, 0x22];
        assert_eq!(scan(&image, &parse("488b").unwrap()), vec![1, 4]);
        assert_eq!(scan(&image, &parse("488b11").unwrap()), vec![1]);
        assert!(scan(&image, &parse("488b33").unwrap()).is_empty());
    }

    #[test]
    fn an_ambiguous_site_is_refused() {
        let image = vec![0u8; 64];
        let site = Site::from_text("twice", 0, "0000").unwrap();
        let err = Moves::locate(&[site], &image).unwrap_err();
        assert!(err.contains("matches 63 places"), "{err}");
    }
}
