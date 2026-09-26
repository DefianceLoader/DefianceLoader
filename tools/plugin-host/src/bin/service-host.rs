fn main() {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap();
    match std::env::args().nth(1).as_deref() {
        Some("reload") => return reload(dir),
        Some("chain") => return chain(dir),
        Some("multiplayer") => return multiplayer(dir),
        Some("add") => return add(dir),
        Some("remove") => return remove(dir),
        Some("toggle") => return toggle(dir),
        _ => {}
    }
    for (id, state) in defiance_loader::test_host::run_plugins(dir) {
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

/// A table as the test host's own consumer (owner 999) holds it.
fn query<T>(
    provider: &core::ffi::CStr,
    name: &core::ffi::CStr,
    version: u32,
) -> Option<&'static T> {
    let dependency = provider.to_str().unwrap().to_string();
    defiance_loader::test_host::begin_services(999, "test", vec![dependency]);
    let table = unsafe {
        (defiance_loader::test_host::service_api().query)(
            provider.as_ptr(),
            name.as_ptr(),
            version,
            core::mem::size_of::<T>(),
        )
    };
    defiance_loader::test_host::finish_services(999, true);
    (!table.is_null()).then(|| unsafe { &*(table as *const T) })
}

#[repr(C)]
struct TotalV1 {
    total: unsafe extern "C" fn() -> u64,
}

/// Counter <- counter-user <- counter-watch (startup-only): reload the
/// counter twice, calling through the watch each time. The watch still holds
/// the old counter-user, which holds the old counter; both must stay mapped.
fn chain(dir: &std::path::Path) {
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    let watch = query::<TotalV1>(c"example.counter-watch", c"watch", 1).expect("the watch service");
    println!("total before: {}", unsafe { (watch.total)() });
    let provider = dir.join("../DefianceLoader/plugins/defiance_example_counter.dll");
    for round in 1..=2 {
        let bytes = std::fs::read(&provider).unwrap();
        std::fs::write(&provider, &bytes).unwrap();
        let result = defiance_loader::test_host::reload_plugin("example.counter");
        println!("reload {round}: {result:?}");
        // A freed copy anywhere down the chain would fault here, unless a new
        // copy happened to map at the old address; the retained list is
        // checked too.
        println!("total after {round}: {}", unsafe { (watch.total)() });
        println!(
            "retained after {round}: {}",
            defiance_loader::test_host::retained_plugins().join(", ")
        );
    }
}

#[repr(C)]
struct MultiplayerV1 {
    blockers: unsafe extern "C" fn(*mut core::ffi::c_char, usize) -> usize,
    guard_installed: unsafe extern "C" fn(),
}

fn blockers(service: &MultiplayerV1) -> String {
    let mut buffer = [0 as core::ffi::c_char; 256];
    unsafe { (service.blockers)(buffer.as_mut_ptr(), buffer.len()) };
    unsafe { core::ffi::CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// The counter is multiplayer-safe at startup; its manifest then says it is
/// not, and it is reloaded: from then on it must block.
fn multiplayer(dir: &std::path::Path) {
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    let service = query::<MultiplayerV1>(c"defiance.loader", c"multiplayer", 1)
        .expect("the multiplayer service");
    unsafe { (service.guard_installed)() };
    println!("blockers before: [{}]", blockers(service));
    let manifest = dir.join("../DefianceLoader/plugins/defiance_example_counter.plugin.json");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        text.replace("\"multiplayer_safe\": true", "\"multiplayer_safe\": false"),
    )
    .unwrap();
    let provider = dir.join("../DefianceLoader/plugins/defiance_example_counter.dll");
    let bytes = std::fs::read(&provider).unwrap();
    std::fs::write(&provider, &bytes).unwrap();
    let result = defiance_loader::test_host::reload_plugin("example.counter");
    println!("reload: {result:?}");
    println!("blockers after: [{}]", blockers(service));
}

fn print_loaded() {
    let ids: Vec<String> = defiance_loader::test_host::loaded_plugins()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    println!("loaded: {}", ids.join(", "));
    println!(
        "retained: {}",
        defiance_loader::test_host::retained_plugins().join(", ")
    );
}

/// The counter at startup; then counter-user's files (waiting in `pending/`
/// beside `bin/`) are dropped into the plugins directory and it is added. Its
/// `service_version` setting was not declared at startup.
fn add(dir: &std::path::Path) {
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    let plugins = dir.join("../DefianceLoader/plugins");
    for entry in std::fs::read_dir(dir.join("../pending")).unwrap().flatten() {
        std::fs::copy(entry.path(), plugins.join(entry.file_name())).unwrap();
    }
    let result = defiance_loader::test_host::add_plugin("example.counter-user");
    println!("add: {result:?}");
    print_loaded();
    if let Some(total) = query::<TotalV1>(c"example.counter-user", c"total", 1) {
        println!("total: {}", unsafe { (total.total)() });
    }
}

/// The counter and counter-user at startup; the counter's files are then
/// deleted and it is removed.
fn remove(dir: &std::path::Path) {
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    let plugins = dir.join("../DefianceLoader/plugins");
    std::fs::remove_file(plugins.join("defiance_example_counter.dll")).unwrap();
    std::fs::remove_file(plugins.join("defiance_example_counter.plugin.json")).unwrap();
    let result = defiance_loader::test_host::remove_plugin("example.counter");
    println!("remove: {result:?}");
    print_loaded();
}

/// Set `enabled` for `id` in the examples group file.
fn set_enabled(dir: &std::path::Path, id: &str, on: bool) {
    let path = dir.join("../DefianceLoader/config/examples.ini");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim_start().starts_with('[') {
            inside = line.trim() == format!("[{id}]");
        }
        if inside && line.trim_start().starts_with("enabled") {
            out.push_str(&format!("enabled = {on}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::write(&path, out).unwrap();
}

/// The counter and counter-user at startup; the counter is switched off in
/// its config file (counter-user, which needs it, goes too) and on again
/// (both come back).
fn toggle(dir: &std::path::Path) {
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    set_enabled(dir, "example.counter", false);
    let result = defiance_loader::test_host::toggle_plugin("example.counter", false);
    println!("off: {result:?}");
    print_loaded();
    set_enabled(dir, "example.counter", true);
    let result = defiance_loader::test_host::toggle_plugin("example.counter", true);
    println!("on: {result:?}");
    print_loaded();
    if let Some(total) = query::<TotalV1>(c"example.counter-user", c"total", 1) {
        println!("total: {}", unsafe { (total.total)() });
    }
}

/// Load, replace the provider's DLL on disk (possible only because the loader
/// runs a copy), reload it, and report who is loaded before and after.
fn reload(dir: &std::path::Path) {
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    let before = defiance_loader::test_host::loaded_plugins();
    let provider = dir.join("../DefianceLoader/plugins/defiance_example_counter.dll");
    let bytes = std::fs::read(&provider).unwrap();
    std::fs::write(&provider, &bytes).expect("the loaded DLL's file is replaceable");
    let result = defiance_loader::test_host::reload_plugin("example.counter");
    println!("reload: {result:?}");
    let after = defiance_loader::test_host::loaded_plugins();
    for (id, owner) in &after {
        let old = before.iter().find(|(b, _)| b == id).map(|(_, o)| *o);
        println!(
            "loaded {id}: {}",
            if old == Some(*owner) { "same" } else { "new" }
        );
    }
}
