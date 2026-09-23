use defiance_api::{Api, Plugin, ABI_VERSION};
use example_counter_contract::CounterV1;
use std::sync::atomic::{AtomicU64, Ordering};

defiance_feature_sdk::service_handshake!();
static VALUE: AtomicU64 = AtomicU64::new(0);
unsafe extern "C" fn get() -> u64 {
    VALUE.load(Ordering::SeqCst)
}
unsafe extern "C" fn increment() -> u64 {
    VALUE.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
}
static COUNTER: CounterV1 = CounterV1 { get, increment };

unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() || (*api).abi_version != ABI_VERSION || (*api).reserved != 0 {
        return 1;
    }
    if defiance_feature_sdk::services::register(c"counter", 1, &COUNTER).is_err() {
        ((*api).log)(
            defiance_api::LOG_ERROR,
            c"counter: service extension unavailable or registration refused".as_ptr(),
        );
        return 1;
    }
    // A deliberate tutorial fault: tests prove this unpublished table is removed.
    match defiance_feature_sdk::boolean(&*api, "example.counter", "fail_init") {
        Ok(false) => 0,
        _ => 1,
    }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"example.counter".as_ptr(),
        version: c"0.1.0".as_ptr(),
        init,
        stop: None,
    })
}
