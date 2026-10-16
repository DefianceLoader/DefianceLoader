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

/// The window in `image` that comes nearest to `pattern`, and the offsets
/// within it of the fixed bytes that differ, for saying why a site is
/// missing. Only windows sharing at least one fixed four-byte run with the
/// pattern are tried; runs of one repeated byte, and runs found too often to
/// say anything, are not used. `None` when no window shares a run.
pub fn closest(image: &[u8], pattern: &[Option<u8>]) -> Option<(usize, Vec<usize>)> {
    const COMMON: usize = 1000;
    if image.len() < pattern.len() || pattern.len() < 4 {
        return None;
    }
    let mut seeds: std::collections::HashMap<[u8; 4], Vec<usize>> = Default::default();
    for k in 0..=pattern.len() - 4 {
        let Some(run) = pattern[k..k + 4]
            .iter()
            .copied()
            .collect::<Option<Vec<u8>>>()
        else {
            continue;
        };
        if run.iter().all(|&b| b == run[0]) {
            continue;
        }
        seeds
            .entry([run[0], run[1], run[2], run[3]])
            .or_default()
            .push(k);
    }
    let mut leads = [false; 256];
    for run in seeds.keys() {
        leads[run[0] as usize] = true;
    }
    let mut hits: std::collections::HashMap<[u8; 4], usize> = Default::default();
    let mut candidates = std::collections::BTreeSet::new();
    let last = image.len() - pattern.len();
    for at in 0..=image.len() - 4 {
        if !leads[image[at] as usize] {
            continue;
        }
        let run = [image[at], image[at + 1], image[at + 2], image[at + 3]];
        let Some(offsets) = seeds.get(&run) else {
            continue;
        };
        let seen = hits.entry(run).or_default();
        *seen += 1;
        if *seen > COMMON {
            continue;
        }
        for &k in offsets {
            if let Some(start) = at.checked_sub(k).filter(|&s| s <= last) {
                candidates.insert(start);
            }
        }
    }
    candidates
        .into_iter()
        .map(|start| {
            let differ = pattern
                .iter()
                .zip(&image[start..start + pattern.len()])
                .enumerate()
                .filter(|(_, (p, &b))| p.is_some_and(|p| p != b))
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            (start, differ)
        })
        .min_by_key(|(_, differ)| differ.len())
}

/// Why `pattern` is not in `image`, as a clause to follow "is not in this
/// build". A window that differs in only a few fixed bytes usually means
/// the code is there but a struct offset or vtable slot in it changed, which
/// is a different build's layout rather than missing code.
fn near_miss(image: &[u8], pattern: &[Option<u8>]) -> String {
    let fixed = pattern.iter().filter(|b| b.is_some()).count();
    match closest(image, pattern) {
        Some((start, differ)) if !differ.is_empty() && differ.len() * 4 <= fixed => {
            let shown = differ
                .iter()
                .take(4)
                .map(|&i| {
                    format!(
                        "+{i:#x} {:02x}>{:02x}",
                        pattern[i].unwrap(),
                        image[start + i]
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let more = if differ.len() > 4 { ", ..." } else { "" };
            format!(
                "; the nearest code, at {start:#x}, differs in {} of its {fixed} fixed bytes ({shown}{more}), which usually means this build's layout changed there",
                differ.len()
            )
        }
        _ => "; nothing in it comes close".into(),
    }
}

/// Where the sites are in this build, as a distance per site.
#[derive(Debug)]
pub struct Moves {
    sites: Vec<Site>,
    deltas: Vec<isize>,
}

impl Moves {
    /// Every site found once, and moved with its group; a missing site is
    /// explained by the nearest code that differs from it.
    pub fn locate(sites: &[Site], image: &[u8]) -> Result<Self, String> {
        Self::find(sites, image).map_err(|(err, missing)| match missing {
            Some(site) => format!("{err}{}", near_miss(image, &site.pattern)),
            None => err,
        })
    }

    /// `locate` without the search for the nearest code, which is the slow
    /// part: for trying a set of sites that is expected to miss. The error
    /// names the first site that failed, and the site itself when it is
    /// missing rather than ambiguous or moved apart.
    pub fn find<'a>(sites: &'a [Site], image: &[u8]) -> Result<Self, (String, Option<&'a Site>)> {
        Self::find_by(sites, |pattern| scan(image, pattern))
    }

    /// `find` with the scan supplied, for trying many sets of sites that
    /// share patterns against one image without scanning it twice for one.
    pub fn find_by<'a>(
        sites: &'a [Site],
        mut scan: impl FnMut(&[Option<u8>]) -> Vec<usize>,
    ) -> Result<Self, (String, Option<&'a Site>)> {
        let mut deltas = Vec::new();
        for site in sites {
            match scan(&site.pattern).as_slice() {
                [one] => deltas.push(*one as isize - site.start as isize),
                [] => {
                    return Err((
                        format!("the {} site is not in this build", site.name),
                        Some(site),
                    ))
                }
                many => {
                    return Err((
                        format!(
                            "the {} site's signature matches {} places in this build",
                            site.name,
                            many.len()
                        ),
                        None,
                    ))
                }
            }
        }
        for (i, a) in sites.iter().enumerate() {
            for (j, b) in sites.iter().enumerate().skip(i + 1) {
                if a.group == b.group && deltas[i] != deltas[j] {
                    return Err((
                        format!(
                            "the {} and {} sites share a function but have moved apart",
                            a.name, b.name
                        ),
                        None,
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

    #[test]
    fn a_missing_site_names_the_bytes_that_changed() {
        let mut image = vec![0u8; 64];
        image[20..32].copy_from_slice(&[
            0x48, 0x8b, 0x86, 0xa8, 0x00, 0x00, 0x00, 0x48, 0x85, 0xc0, 0x74, 0x10,
        ]);
        let site = Site::from_text("moved", 0, "488b86a0000000 4885c0 74??").unwrap();
        let err = Moves::locate(&[site], &image).unwrap_err();
        assert!(err.contains("at 0x14, differs in 1 of its 11"), "{err}");
        assert!(err.contains("+0x3 a0>a8"), "{err}");
    }

    #[test]
    fn a_site_with_nothing_near_says_so() {
        let image: Vec<u8> = (0..=255).collect();
        let site = Site::from_text("gone", 0, "488b86a0000000").unwrap();
        let err = Moves::locate(&[site], &image).unwrap_err();
        assert!(err.ends_with("nothing in it comes close"), "{err}");
    }
}
