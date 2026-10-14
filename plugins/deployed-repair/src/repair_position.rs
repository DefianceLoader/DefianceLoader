//! Crew repair positioning accepts units without a wheels-script descriptor.
use defiance_core::sites::{sig, Image, Signature};
use std::sync::atomic::AtomicUsize;

pub(crate) static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
pub(crate) static RESUME: AtomicUsize = AtomicUsize::new(0);
pub(crate) static POINTS_OFFSET: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Site {
    pub site: usize,
    pub offset: usize,
}

impl Site {
    pub fn before(self) -> [u8; 14] {
        let mut bytes = [0x48, 0x8b, 0x8e, 0, 0, 0, 0, 0x48, 0x8b, 0x86, 0, 0, 0, 0];
        bytes[3..7].copy_from_slice(&(self.offset as u32).to_le_bytes());
        bytes[10..14].copy_from_slice(&((self.offset + 8) as u32).to_le_bytes());
        bytes
    }
}

// RSI is the nullable WheelsUnitScriptInfo cast; R13D is the crew index.
// The native count comparison selects the bounding-box path for an empty
// vector and initializes both XMM constants before either positioning path.
const POINTS: Signature = sig(
    "crew repair point selection",
    &[(
        concat!(
            "488b03488bcbff90b00000004c8b38",
            "488b8e??090000488b86??090000482bc148c1f807",
            "48baabaaaaaaaaaaaaaa480fafc2f30f103d????????450f57c0",
            "4963d5483bd00f8385000000488d045248c1e007",
            "f30f108408a4000000",
        ),
        0xf,
    )],
);

const BOUNDS: Signature = sig(
    "crew repair bounding-box fallback",
    &[(
        concat!(
            "488bcbe8????????488b084c8b4178488d55e7488bc841ffd090",
            "488b08488b01ff9038010000",
        ),
        0,
    )],
);

pub(crate) fn resolve(image: &Image<'_>) -> Result<Site, String> {
    let site = image.find(&POINTS)?;
    let offset = image.u32(site + 3).unwrap_or_default() as usize;
    if ![0x970, 0x988].contains(&offset) || image.u32(site + 10) != Some((offset + 8) as u32) {
        return Err("crew repair point vector layout differs".into());
    }
    if image.find(&BOUNDS)? != site + 0xc0 {
        return Err("crew repair bounding-box fallback moved outside its selector".into());
    }
    let resolved = Site { site, offset };
    image.expect(POINTS.name, site, &resolved.before())?;
    Ok(resolved)
}

/// A missing wheels descriptor has no explicit repair points. Resume the
/// native count test so it computes positions from the unit's geometry bounds,
/// including its normal facing, crew spacing and repair animation setup.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn points() {
    core::arch::naked_asm!(
        "test rsi, rsi",
        "jz 2f",
        "mov rax, qword ptr [rip+{offset}]",
        "mov rcx, [rsi+rax]",
        "mov rax, [rsi+rax+8]",
        "jmp qword ptr [rip+{resume}]",
        "2:",
        "xor ecx, ecx",
        "xor eax, eax",
        "jmp qword ptr [rip+{resume}]",
        offset = sym POINTS_OFFSET,
        resume = sym RESUME,
    );
}

#[cfg(feature = "parity-test")]
#[no_mangle]
pub extern "C" fn deployed_repair_test_position_detour(resume: usize, offset: usize) -> usize {
    RESUME.store(resume, std::sync::atomic::Ordering::Release);
    POINTS_OFFSET.store(offset, std::sync::atomic::Ordering::Release);
    points as *const () as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    #[test]
    fn resolves_native_point_selection_on_all_builds() {
        let expected = [
            0xe833e, 0xe83ce, 0xef5ce, 0xef65e, 0xef5ce, 0xef65e, 0xef5ce, 0xef65e,
        ];
        for (build, site) in BUILDS.into_iter().zip(expected) {
            let Some(mapped) = reference(build, "logic.dll") else {
                continue;
            };
            assert_eq!(
                resolve(&Image::mapped(&mapped)),
                Ok(Site {
                    site,
                    offset: if build.contains("2025-12-23") {
                        0x970
                    } else {
                        0x988
                    },
                }),
                "{build}"
            );
        }
    }

    #[test]
    fn refuses_changed_point_vector_and_bounds_path() {
        let Some(mapped) = reference(BUILDS[0], "logic.dll") else {
            return;
        };
        let site = resolve(&Image::mapped(&mapped)).unwrap().site;
        for offset in [0, 3, 10, 0x12, 0x2b, 0x35, 0xc0, 0xda] {
            let mut changed = mapped.image.clone();
            changed[site + offset] ^= 1;
            assert!(resolve(&Image {
                image: &changed,
                base: mapped.base
            })
            .is_err());
        }
    }
}
