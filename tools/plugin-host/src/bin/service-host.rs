fn main() {
    let exe = std::env::current_exe().unwrap();
    for (id, state) in defiance_loader::test_host::run_plugins(exe.parent().unwrap()) {
        println!("{id}: {state}");
    }
    // Stopping removes discovery even though DLL code/storage remains mapped.
    defiance_loader::test_host::begin_services(999, "test", vec!["example.counter".into()]);
    let table = unsafe {
        (defiance_loader::test_host::service_api().query)(
            c"example.counter".as_ptr(),
            c"counter".as_ptr(),
            1,
            1,
        )
    };
    assert!(table.is_null());
    defiance_loader::test_host::finish_services(999, false);
}
