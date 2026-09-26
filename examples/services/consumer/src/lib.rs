use defiance_api::{Api, Plugin, ABI_VERSION, LOG_ERROR, LOG_INFO};
use example_counter_contract::{CounterV1, TotalV1};
use std::sync::OnceLock;
defiance_feature_sdk::service_handshake!();

/// The counter's table, cached as a consumer does, for `total`.
static COUNTER: OnceLock<&'static CounterV1> = OnceLock::new();
unsafe extern "C" fn total() -> u64 {
    COUNTER
        .get()
        .map_or(0, |counter| unsafe { (counter.get)() })
}
static TOTAL: TotalV1 = TotalV1 { total };

unsafe extern "C" fn init(api: *const Api) -> i32 {
    if api.is_null() || (*api).abi_version != ABI_VERSION || (*api).reserved != 0 {
        return 1;
    }
    let version =
        match defiance_feature_sdk::integer(&*api, "example.counter-user", "service_version") {
            Ok(v) => v as u32,
            Err(_) => return 1,
        };
    let Some(counter) =
        defiance_feature_sdk::services::query::<CounterV1>(c"example.counter", c"counter", version)
    else {
        ((*api).log)(
            LOG_ERROR,
            c"counter-user: required counter service unavailable".as_ptr(),
        );
        return 1;
    };
    let before = (counter.get)();
    if (counter.increment)() != before.wrapping_add(1)
        || (counter.increment)() != before.wrapping_add(2)
        || (counter.get)() != before.wrapping_add(2)
    {
        return 1;
    }
    ((*api).log)(
        LOG_INFO,
        c"counter-user: shared state verified (two increments)".as_ptr(),
    );
    let _ = COUNTER.set(counter);
    if defiance_feature_sdk::services::register(c"total", 1, &TOTAL).is_err() {
        return 1;
    }
    0
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"example.counter-user".as_ptr(),
        version: c"0.1.0".as_ptr(),
        init,
        stop: None,
    })
}
