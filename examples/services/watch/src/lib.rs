//! The third link of a service chain: holds `example.counter-user`'s `total`
//! table and offers its own `watch`, which calls through it to the counter.
//! Loaded as a startup-only plugin (`hot_reload: false`), it is the holder a
//! hot reload of the counter must keep the old copies alive for.
use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR};
use example_counter_contract::TotalV1;
use std::sync::OnceLock;
defiance_feature_sdk::service_handshake!();

static USER: OnceLock<&'static TotalV1> = OnceLock::new();
unsafe extern "C" fn total() -> u64 {
    USER.get().map_or(0, |user| unsafe { (user.total)() })
}
static WATCH: TotalV1 = TotalV1 { total };

unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() || (*api).abi_version != ABI_VERSION || (*api).reserved != 0 {
        return 1;
    }
    let Some(user) =
        defiance_feature_sdk::services::query::<TotalV1>(c"example.counter-user", c"total", 1)
    else {
        ((*api).log)(
            LOG_ERROR,
            c"counter-watch: the total service is unavailable".as_ptr(),
        );
        return 1;
    };
    let _ = USER.set(user);
    if defiance_feature_sdk::services::register(c"watch", 1, &WATCH).is_err() {
        return 1;
    }
    0
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"example.counter-watch".as_ptr(),
        version: c"0.1.0".as_ptr(),
        init,
        stop: None,
    })
}
