defiance_feature_sdk::crash_handshake!();
#[no_mangle]
pub extern "C" fn defiance_test_panic() {
    panic!("intentional DLL panic regression");
}
