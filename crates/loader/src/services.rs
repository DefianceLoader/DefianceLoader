//! Init-time discovery of provider-owned, versioned function tables.
use core::ffi::{c_char, c_void, CStr};
use defiance_api::ServiceApiV1;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

#[derive(Clone)]
struct Context {
    owner: usize,
    id: String,
    dependencies: Vec<String>,
}
struct Service {
    owner: usize,
    address: usize,
    size: usize,
    active: bool,
}
#[derive(Default)]
struct Registry {
    tables: BTreeMap<(String, String, u32), Service>,
}

impl Registry {
    fn register(
        &mut self,
        context: &Context,
        name: String,
        version: u32,
        address: usize,
        size: usize,
    ) -> i32 {
        if version == 0 || address == 0 || size == 0 {
            return 1;
        }
        let key = (context.id.clone(), name, version);
        if self.tables.contains_key(&key) {
            return 3;
        }
        self.tables.insert(
            key,
            Service {
                owner: context.owner,
                address,
                size,
                active: false,
            },
        );
        0
    }
    fn query(
        &self,
        context: &Context,
        provider: &str,
        name: &str,
        version: u32,
        size: usize,
    ) -> usize {
        if size == 0 || !context.dependencies.iter().any(|id| id == provider) {
            return 0;
        }
        self.tables
            .get(&(provider.to_owned(), name.to_owned(), version))
            .filter(|s| s.active && s.size >= size)
            .map_or(0, |s| s.address)
    }
    fn finish(&mut self, owner: usize, success: bool) {
        if success {
            for table in self.tables.values_mut().filter(|s| s.owner == owner) {
                table.active = true;
            }
        } else {
            self.tables.retain(|_, s| s.owner != owner);
        }
    }
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
thread_local! { static CURRENT: RefCell<Option<Context>> = const { RefCell::new(None) }; }
fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

pub fn begin(owner: usize, id: &str, dependencies: Vec<String>) {
    CURRENT.with(|slot| {
        *slot.borrow_mut() = Some(Context {
            owner,
            id: id.to_ascii_lowercase(),
            dependencies: dependencies
                .into_iter()
                .map(|id| id.to_ascii_lowercase())
                .collect(),
        })
    });
}
pub fn finish(owner: usize, success: bool) {
    CURRENT.with(|slot| *slot.borrow_mut() = None);
    registry().lock().unwrap().finish(owner, success);
}
/// Drop every table a plugin registered. Used by the in-process test host's
/// unload path; the shipping loader only removes on a failed `init`.
#[cfg(feature = "test-host")]
pub fn remove(owner: usize) {
    registry().lock().unwrap().finish(owner, false);
}

unsafe fn name(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let bytes = unsafe { CStr::from_ptr(ptr) }.to_bytes();
    if bytes.is_empty()
        || bytes.len() > 128
        || !bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(b))
    {
        return None;
    }
    Some(unsafe { core::str::from_utf8_unchecked(bytes) }.to_ascii_lowercase())
}
unsafe extern "C" fn register(
    name_ptr: *const c_char,
    version: u32,
    table: *const c_void,
    size: usize,
) -> i32 {
    let Some(name) = (unsafe { name(name_ptr) }) else {
        return 1;
    };
    CURRENT.with(|slot| {
        let context = slot.borrow();
        let Some(context) = context.as_ref() else {
            return 2;
        };
        registry()
            .lock()
            .unwrap()
            .register(context, name, version, table as usize, size)
    })
}
unsafe extern "C" fn query(
    provider: *const c_char,
    service: *const c_char,
    version: u32,
    size: usize,
) -> *const c_void {
    let (Some(provider), Some(service)) = (unsafe { name(provider) }, unsafe { name(service) })
    else {
        return core::ptr::null();
    };
    CURRENT.with(|slot| {
        let context = slot.borrow();
        let Some(context) = context.as_ref() else {
            return core::ptr::null();
        };
        registry()
            .lock()
            .unwrap()
            .query(context, &provider, &service, version, size) as *const c_void
    })
}

pub static API: ServiceApiV1 = ServiceApiV1 {
    version: 1,
    size: core::mem::size_of::<ServiceApiV1>() as u32,
    register,
    query,
};

#[cfg(test)]
mod tests {
    use super::*;
    fn context(owner: usize, id: &str, dependencies: &[&str]) -> Context {
        Context {
            owner,
            id: id.into(),
            dependencies: dependencies.iter().map(|s| s.to_string()).collect(),
        }
    }
    #[test]
    fn publication_version_size_dependency_and_cleanup() {
        let mut r = Registry::default();
        let provider = context(1, "provider", &[]);
        let consumer = context(2, "consumer", &["provider"]);
        assert_eq!(r.register(&provider, "service".into(), 1, 123, 16), 0);
        assert_eq!(r.query(&consumer, "provider", "service", 1, 16), 0);
        r.finish(1, true);
        assert_eq!(r.query(&consumer, "provider", "service", 1, 16), 123);
        assert_eq!(r.query(&consumer, "provider", "service", 2, 16), 0);
        assert_eq!(r.query(&consumer, "provider", "service", 1, 17), 0);
        assert_eq!(r.query(&provider, "provider", "service", 1, 16), 0);
        assert_eq!(r.register(&provider, "service".into(), 1, 456, 16), 3);
        r.finish(1, false);
        assert_eq!(r.query(&consumer, "provider", "service", 1, 16), 0);
    }
    #[test]
    fn failed_init_discards_all_versions_and_other_providers_survive() {
        let mut r = Registry::default();
        let a = context(1, "a", &[]);
        let b = context(2, "b", &["a"]);
        for version in [1, 2] {
            assert_eq!(r.register(&a, "x".into(), version, 123, 8), 0);
        }
        assert_eq!(r.register(&b, "x".into(), 1, 456, 8), 0);
        r.finish(2, true);
        r.finish(1, false);
        assert_eq!(r.tables.len(), 1);
        assert_eq!(r.query(&b, "a", "x", 1, 8), 0);
    }
    #[test]
    fn registration_requires_valid_table_and_init_thread() {
        let mut r = Registry::default();
        let context = context(1, "a", &[]);
        for (version, address, size) in [(0, 1, 8), (1, 0, 8), (1, 1, 0)] {
            assert_eq!(r.register(&context, "x".into(), version, address, size), 1);
        }
        begin(987, "test", vec![]);
        assert_eq!(
            std::thread::spawn(|| unsafe {
                register(b"x\0".as_ptr().cast(), 1, 1usize as *const c_void, 8)
            })
            .join()
            .unwrap(),
            2
        );
        finish(987, false);
        assert_eq!(
            unsafe { register(b"x\0".as_ptr().cast(), 1, 1usize as *const c_void, 8) },
            2
        );
    }
    #[test]
    fn names_are_canonical_and_invalid_inputs_are_refused() {
        assert_eq!(
            unsafe { name(c"Counter.V1".as_ptr()) }.as_deref(),
            Some("counter.v1")
        );
        assert!(unsafe { name(c"".as_ptr()) }.is_none());
        assert!(unsafe { name(c"bad/name".as_ptr()) }.is_none());
        assert!(unsafe { name(core::ptr::null()) }.is_none());
        let long = std::ffi::CString::new("x".repeat(129)).unwrap();
        assert!(unsafe { name(long.as_ptr()) }.is_none());
        assert!(unsafe { query(c"provider".as_ptr(), c"counter".as_ptr(), 1, 8) }.is_null());
    }
}
