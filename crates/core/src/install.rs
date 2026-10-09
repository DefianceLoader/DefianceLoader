//! Whether a running build's sites have to be found by signature. Shared so
//! the injector and the loader's Core plugin make the same decision; the
//! injector's install sequence around it is [`crate::compat::install`].

/// When to find the sites by signature rather than use them as built.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Scan {
    /// never for the build the patch was written for; always for a verified one
    Default,
    /// --scan: also for any other build
    Unknown,
    /// --force-scan: for every build, the one it was written for included
    Always,
}

/// Whether this build's sites have to be found by signature rather than used as
/// built. `sha` is the module on disk, `source` the build the patch was written
/// for, `verified` the other builds already checked.
pub fn needs_relocation(sha: &str, source: &str, verified: &[String], scan: Scan) -> bool {
    if scan == Scan::Always {
        return true;
    }
    if sha == source {
        return false;
    }
    verified.iter().any(|v| v == sha) || scan == Scan::Unknown
}
