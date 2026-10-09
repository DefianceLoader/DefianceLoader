//! Core's `facets` service: an entity's typed facets, as logic.dll's own
//! accessors answer them.
//!
//! logic.dll's squad-AI accessor asks the entity for its facet record (vtable
//! +0xb0), reads the AI facet (+0x28) and returns it only when the facet's
//! runtime type (its vtable +0x20) is `SquadAiFacet` or derives from it, the
//! type handle coming from `SquadAiFacet`'s static type getter. Two other
//! accessors share that body with another type's getter, so [`resolve`] picks
//! the one that calls the getter `SquadAiFacet`'s own type method calls. The
//! table calls that accessor, after checking the entity and the facet record
//! the native code dereferences unchecked.
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use defiance_api::FacetsV1;
use defiance_core::sites::Image;

const SQUAD_AI_FACET: &str = ".?AVSquadAiFacet@Leonardo@@";
/// The facet's runtime-type method, as a primary-vtable slot.
const TYPE_SLOT: usize = 4;
/// That method's body up to its call of the class's type getter:
/// `rex push rbx; sub rsp, 0x20; mov rcx, rdx; mov rbx, rdx`.
const TYPE_METHOD: [u8; 12] = [
    0x40, 0x53, 0x48, 0x83, 0xec, 0x20, 0x48, 0x8b, 0xca, 0x48, 0x8b, 0xda,
];
/// The facet accessors' shared body; the type getter's call is at
/// [`GETTER_CALL`].
const ACCESSOR: &str = "48895c2418574883ec20488b01ff90b0000000488b78284885ff744f\
    488d4c2430e8????????488d542438488bcf488b18488b07ff5020488b088b4350394150\
    7229483bcb740c488b5128488bca4885d275ef4885c97413483bcb750e488bc7488b5c24\
    404883c4205fc3";
const GETTER_CALL: usize = 0x21;
/// The entity's facet-record getter, as a vtable offset.
const ENTITY_FACETS: usize = 0xb0;

/// The rva of the build's squad-AI accessor.
pub(crate) fn resolve(image: &Image) -> Result<usize, String> {
    let method = image.method(SQUAD_AI_FACET, TYPE_SLOT)?;
    image.expect("SquadAiFacet type method", method, &TYPE_METHOD)?;
    let getter = image
        .branch_target(method + TYPE_METHOD.len())
        .ok_or("SquadAiFacet type method calls no type getter")?;
    let pattern = defiance_core::pattern::parse(ACCESSOR)?;
    let found: Vec<usize> = defiance_core::scan::scan(image.image, &pattern)
        .into_iter()
        .filter(|&hit| image.branch_target(hit + GETTER_CALL) == Some(getter))
        .collect();
    match found[..] {
        [accessor] => Ok(accessor),
        [] => Err("no squad AI accessor".into()),
        _ => Err(format!("{} squad AI accessors", found.len())),
    }
}

/// [`resolve`] on the loaded logic.dll, read from the original view, and
/// the table armed with the accessor's address.
pub(crate) fn resolve_loaded(api: &defiance_api::Api) -> Result<(), String> {
    let base = unsafe { (api.module_base)(c"logic.dll".as_ptr()) } as *const u8;
    if base.is_null() {
        return Err("logic.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base.cast_mut().cast()) };
    let image = crate::plan::original_image(base, size).ok_or("logic.dll is unreadable")?;
    let accessor = resolve(&Image {
        image: &image,
        base: base as usize,
    })?;
    SQUAD_AI.store(base as usize + accessor, Ordering::Release);
    Ok(())
}

/// The native squad-AI accessor's address; zero until resolved.
static SQUAD_AI: AtomicUsize = AtomicUsize::new(0);

unsafe fn method(object: *mut c_void, offset: usize) -> usize {
    unsafe { *(*object.cast::<*const usize>()).add(offset / 8) }
}

/// Whether the entity is non-null and has a facet record, the one
/// dereference the native accessors leave unchecked.
unsafe fn has_facets(entity: *mut c_void) -> bool {
    if entity.is_null() {
        return false;
    }
    let facets: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
        unsafe { core::mem::transmute(method(entity, ENTITY_FACETS)) };
    !unsafe { facets(entity) }.is_null()
}

unsafe extern "C" fn squad_ai(entity: *mut c_void) -> *mut c_void {
    let accessor = SQUAD_AI.load(Ordering::Acquire);
    if accessor == 0 || !unsafe { has_facets(entity) } {
        return core::ptr::null_mut();
    }
    let accessor: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
        unsafe { core::mem::transmute(accessor) };
    unsafe { accessor(entity) }
}

pub static API: FacetsV1 = FacetsV1 { squad_ai };

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_core::sites::{reference, BUILDS};

    unsafe extern "C" fn record_of(entity: *mut c_void) -> *mut c_void {
        unsafe { *entity.cast::<*mut c_void>().add(1) }
    }

    /// Stands in for the native accessor: the record's AI facet.
    unsafe extern "C" fn fake_accessor(entity: *mut c_void) -> *mut c_void {
        unsafe { *record_of(entity).cast::<*mut c_void>().add(5) }
    }

    #[test]
    fn null_entities_and_records_never_reach_the_accessor() {
        SQUAD_AI.store(fake_accessor as *const () as usize, Ordering::Release);
        let mut vtable = vec![0usize; 32];
        vtable[ENTITY_FACETS / 8] = record_of as *const () as usize;
        let ai = 0x1234usize;
        let mut record = [0usize; 8];
        record[5] = ai;
        let entity = [vtable.as_ptr() as usize, record.as_ptr() as usize];
        let missing = [vtable.as_ptr() as usize, 0];
        unsafe {
            assert!(squad_ai(core::ptr::null_mut()).is_null());
            assert!(squad_ai(missing.as_ptr() as *mut c_void).is_null());
            assert_eq!(squad_ai(entity.as_ptr() as *mut c_void) as usize, ai);
        }
    }

    /// The squad-AI accessor's rva, per build.
    const ACCESSOR_RVA: [(&str, usize); 8] = [
        ("gog/2025-12-23", 0x9ae20),
        ("steam/2025-12-23", 0x9aeb0),
        ("gog/2026-09-14", 0xa30f0),
        ("steam/2026-09-22", 0xa3180),
        ("gog/2026-09-25", 0xa30f0),
        ("steam/2026-09-25", 0xa3180),
        ("gog/2026-10-07", 0xa30f0),
        ("steam/2026-10-07", 0xa3180),
    ];

    #[test]
    fn the_accessor_resolves_on_every_build_in_bin() {
        assert_eq!(ACCESSOR_RVA.map(|(build, _)| build), BUILDS);
        for (build, rva) in ACCESSOR_RVA {
            let Some(mapped) = reference(build, "logic.dll") else {
                continue;
            };
            assert_eq!(resolve(&Image::mapped(&mapped)), Ok(rva), "{build}");
        }
    }

    #[test]
    fn a_changed_accessor_or_type_method_is_refused() {
        let (build, rva) = ACCESSOR_RVA[0];
        let Some(mapped) = reference(build, "logic.dll") else {
            return;
        };
        let method = Image::mapped(&mapped)
            .method(SQUAD_AI_FACET, TYPE_SLOT)
            .unwrap();
        // The facet-record and AI-facet offsets, and the getter call.
        for at in [rva + 0x0f, rva + 0x16, rva + GETTER_CALL + 1, method + 13] {
            let mut changed = mapped.image.clone();
            changed[at] ^= 8;
            let image = Image {
                image: &changed,
                base: mapped.base,
            };
            assert!(resolve(&image).is_err(), "byte {at:#x}");
        }
    }
}
