//! Game build hashes and the final deny branch in `SmartCursorHacking`.

pub(super) struct Build {
    pub(super) name: &'static str,
    pub(super) sha: &'static str,
    pub(super) deny_branch: usize,
}

pub(super) const BUILDS: &[Build] = &[
    Build {
        name: "GOG 2025-12-23",
        sha: "f0184b9fe358172c83261419c8ba3d822a0aa6b06ed3cddb2f7aa3ebb9653db4",
        deny_branch: 0x3373c5,
    },
    Build {
        name: "GOG 2026-09-14",
        sha: "bc2af42369f9f6fe70e206ae4846f8f0e46ac01cca9a325c8159ee04a9b5e405",
        deny_branch: 0x3395a5,
    },
    Build {
        name: "GOG 2026-09-25",
        sha: "8ec30a0b59aebf2240f00229d54a0f58e2e338f9ab3511046c9ff36970dd1489",
        deny_branch: 0x3395a5,
    },
    Build {
        name: "Steam 2025-12-23",
        sha: "dc10419f417aed4ecff348b7c96b3c7c574a9a2541f76c5fbc35eb92dbc716d6",
        deny_branch: 0x33d875,
    },
    Build {
        name: "Steam 2026-09-22",
        sha: "d926a213731d73bac8ccc56b50e2b4292fe8613c7a9bb7c8b9fc9132c122ed25",
        deny_branch: 0x33fa85,
    },
    Build {
        name: "Steam 2026-09-25",
        sha: "c336b5ed4a367628a9c370457d82b1d4e75cab9007cffe5354e27688e836f98e",
        deny_branch: 0x33fa85,
    },
];
