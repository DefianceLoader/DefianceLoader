#[cfg(feature = "patch-v1-test")]
#[path = "../patch_v1_test.rs"]
mod patch_v1_test;

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
        Some("settings") => return settings(dir),
        Some("live-enable-settings") => return live_enable_settings(dir),
        Some("live-enable-reload") => return live_enable_reload(dir),
        Some("revoke-target") => return revoke_reload_permission(dir, true),
        Some("revoke-consumer") => return revoke_reload_permission(dir, false),
        Some("mixed-reload-config") => return mixed_reload_config(dir),
        Some("startup-retry") => return startup_retry(dir, "normal"),
        Some("startup-retry-repeat") => return startup_retry(dir, "repeat"),
        Some("startup-retry-moved") => return startup_retry(dir, "moved"),
        Some("startup-retry-disabled") => return startup_retry_refusal(dir, "disabled"),
        Some("startup-retry-revoked") => return startup_retry_refusal(dir, "revoked"),
        Some("startup-retry-legacy") => return startup_retry_refusal(dir, "legacy"),
        Some("startup-retry-superseded") => return startup_retry_superseded(dir),
        #[cfg(feature = "patch-v1-test")]
        Some("patch-v1") => return patch_v1_test::run(),
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
    let result = defiance_loader::test_host::resettle_plugin("example.counter-user");
    println!("resettle added: {result:?}");
    print_loaded();

    let manifest = plugins.join("defiance_example_counter_user.plugin.json");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let changed = text.replacen(
        "\"settings\": [",
        "\"settings\":[{\"key\":\"audit_marker\",\"type\":\"bool\",\"default\":\"false\",\"description\":\"Audit marker.\"},",
        1,
    );
    assert_ne!(changed, text, "the fixture manifest has a settings array");
    std::fs::write(manifest, changed).unwrap();
    let result = defiance_loader::test_host::resettle_plugin("example.counter-user");
    println!("resettle changed schema: {result:?}");
    print_loaded();
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
    set_value(dir, id, "enabled", &on.to_string());
}

/// Set `key` for `id` in the examples group file.
fn set_value(dir: &std::path::Path, id: &str, key: &str, value: &str) {
    let path = dir.join("../DefianceLoader/config/examples.ini");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim_start().starts_with('[') {
            inside = line.trim() == format!("[{id}]");
        }
        if inside && line.split('=').next().map(str::trim) == Some(key) {
            out.push_str(&format!("{key} = {value}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::write(&path, out).unwrap();
}

/// Retry a failed startup provider only after its DLL changes. The repeat
/// variant proves another changed candidate can fail and then recover.
fn startup_retry(dir: &std::path::Path, mode: &str) {
    let repeat_failure = mode == "repeat";
    let moved_dll = mode == "moved";
    std::fs::write(
        dir.join("../DefianceLoader/config/examples.ini"),
        "[example.counter]\nfail_init = true\n",
    )
    .unwrap();
    let initial: Vec<_> = defiance_loader::test_host::load_plugins(dir)
        .into_iter()
        .collect();
    println!("startup: {initial:?}");
    assert!(initial
        .iter()
        .any(|(id, state)| { id == "example.counter" && state.starts_with("Failed") }));
    assert!(initial
        .iter()
        .any(|(id, state)| { id == "example.counter-user" && state.starts_with("Blocked") }));
    assert!(!defiance_loader::test_host::loaded_plugins()
        .iter()
        .any(|(id, _)| id == "example.counter"));

    let poll = || defiance_loader::test_host::poll_startup_retries();
    assert!(
        poll().is_empty(),
        "a failed plugin is not retried without a DLL change"
    );
    set_value(dir, "example.counter", "fail_init", "false");
    let unchanged = defiance_loader::test_host::toggle_plugin("example.counter", true);
    assert!(
        unchanged.is_err(),
        "config enable cannot bypass the settled DLL retry path"
    );
    assert!(defiance_loader::test_host::loaded_plugins().is_empty());
    assert!(
        poll().is_empty(),
        "config-only correction does not replace the DLL"
    );
    if repeat_failure {
        set_value(dir, "example.counter", "fail_init", "true");
    }

    let plugins = dir.join("../DefianceLoader/plugins");
    let mut provider = plugins.join("defiance_example_counter.dll");
    if moved_dll {
        let moved_provider = plugins.join("defiance_example_counter_moved.dll");
        let manifest = plugins.join("defiance_example_counter.plugin.json");
        let moved_manifest = plugins.join("defiance_example_counter_moved.plugin.json");
        std::fs::rename(&provider, &moved_provider).unwrap();
        std::fs::rename(&manifest, &moved_manifest).unwrap();
        let text = std::fs::read_to_string(&moved_manifest).unwrap();
        let changed = text.replace(
            "\"dll\": \"defiance_example_counter.dll\"",
            "\"dll\": \"defiance_example_counter_moved.dll\"",
        );
        assert_ne!(changed, text, "the managed manifest names its DLL");
        std::fs::write(&moved_manifest, changed).unwrap();
        provider = moved_provider;

        let enabled = defiance_loader::test_host::toggle_plugin("example.counter", true);
        println!("ordinary enable after move: {enabled:?}");
        assert!(
            enabled.is_err(),
            "ordinary config enable cannot bypass the failed-startup ID guard after a DLL move"
        );
        assert!(!defiance_loader::test_host::loaded_plugins()
            .iter()
            .any(|(id, _)| id == "example.counter"));
        assert!(
            defiance_loader::test_host::plugin_summary().contains("1 failed"),
            "ordinary enable preserves the startup failure"
        );
    }
    let append_overlay = || {
        let mut bytes = std::fs::read(&provider).unwrap();
        bytes.push(0);
        std::fs::write(&provider, bytes).unwrap();
    };
    let apply = || {
        defiance_loader::test_host::apply_pending_changes();
        let loaded = defiance_loader::test_host::loaded_plugins();
        let active = loaded.iter().any(|(id, _)| id == "example.counter");
        println!("loaded after apply: {loaded:?}");
        println!(
            "summary after apply: {}",
            defiance_loader::test_host::plugin_summary()
        );
        active
    };

    if repeat_failure {
        append_overlay();
        assert!(
            poll().is_empty(),
            "first observation only starts the stability window"
        );
        assert_eq!(poll(), vec!["example.counter".to_string()]);
        assert!(
            !defiance_loader::test_host::loaded_plugins()
                .iter()
                .any(|(id, _)| id == "example.counter"),
            "discovery must not initialize before the safe-boundary drain"
        );
        assert!(
            !apply(),
            "the unchanged failing configuration must fail again"
        );
        assert!(
            defiance_loader::test_host::plugin_summary().contains("1 failed"),
            "a second failure remains visible in the fresh summary"
        );
        assert!(
            poll().is_empty(),
            "the same failed DLL stamp must not queue another retry"
        );
    }

    set_value(dir, "example.counter", "fail_init", "false");
    append_overlay();
    assert!(
        poll().is_empty(),
        "first observation only starts the stability window"
    );
    assert_eq!(poll(), vec!["example.counter".to_string()]);
    assert!(
        !defiance_loader::test_host::loaded_plugins()
            .iter()
            .any(|(id, _)| id == "example.counter"),
        "discovery must not initialize before the safe-boundary drain"
    );
    assert!(apply(), "the corrected candidate should become active");
    let summary = defiance_loader::test_host::plugin_summary();
    assert!(
        summary.contains("0 failed"),
        "recovery clears the stale failure: {summary}"
    );
    assert!(
        !defiance_loader::test_host::loaded_plugins()
            .iter()
            .any(|(id, _)| id == "example.counter-user"),
        "a dependant blocked at startup is not implicitly started by provider retry"
    );
}

/// A fresh disabled or non-reloadable candidate cannot retry a startup failure.
fn startup_retry_refusal(dir: &std::path::Path, refusal: &str) {
    let config = dir.join("../DefianceLoader/config/examples.ini");
    std::fs::write(&config, "[example.counter]\nfail_init = true\n").unwrap();
    let initial = defiance_loader::test_host::load_plugins(dir);
    println!("startup: {initial:?}");
    assert!(initial
        .iter()
        .any(|(id, state)| { id == "example.counter" && state.starts_with("Failed") }));
    let poll = || defiance_loader::test_host::poll_startup_retries();
    assert!(poll().is_empty());

    let plugins = dir.join("../DefianceLoader/plugins");
    let provider = plugins.join("defiance_example_counter.dll");
    match refusal {
        "disabled" => std::fs::write(
            &config,
            "[example.counter]\nfail_init = false\nenabled = false\n",
        )
        .unwrap(),
        "revoked" => {
            std::fs::write(&config, "[example.counter]\nfail_init = false\n").unwrap();
            let sidecar = plugins.join("defiance_example_counter.plugin.json");
            let text = std::fs::read_to_string(&sidecar).unwrap();
            let changed = text.replace("\"hot_reload\": true", "\"hot_reload\": false");
            assert_ne!(
                changed, text,
                "the provider manifest declares hot_reload true"
            );
            std::fs::write(sidecar, changed).unwrap();
        }
        "legacy" => {
            std::fs::write(&config, "[example.counter]\nfail_init = false\n").unwrap();
            std::fs::remove_file(plugins.join("defiance_example_counter.plugin.json")).unwrap()
        }
        _ => panic!("unknown startup retry refusal: {refusal}"),
    }
    let mut bytes = std::fs::read(&provider).unwrap();
    bytes.push(0);
    std::fs::write(&provider, bytes).unwrap();
    assert!(
        poll().is_empty(),
        "the first changed observation is unsettled"
    );
    if refusal == "legacy" {
        assert!(poll().is_empty(), "legacy discovery has a different ID");
        let addition = defiance_loader::test_host::add_plugin("defiance_example_counter");
        assert!(
            addition.unwrap_err().contains("failed startup plugin"),
            "deleting the sidecar cannot bypass recovery policy as a legacy addition"
        );
    } else {
        assert_eq!(poll(), vec!["example.counter".to_string()]);
    }
    defiance_loader::test_host::apply_pending_changes();
    let loaded = defiance_loader::test_host::loaded_plugins();
    let summary = defiance_loader::test_host::plugin_summary();
    println!("{refusal} loaded after apply: {loaded:?}");
    println!("{refusal} summary after apply: {summary}");
    assert!(!loaded.iter().any(|(id, _)| id == "example.counter"));
    assert!(
        summary.contains("0 active"),
        "candidate stayed inactive: {summary}"
    );
    if refusal == "disabled" {
        assert!(
            summary.contains("1 disabled"),
            "fresh disabled config is reflected: {summary}"
        );
    }
}

/// A queued candidate becomes stale if a newer DLL stamp arrives before apply.
fn startup_retry_superseded(dir: &std::path::Path) {
    let config = dir.join("../DefianceLoader/config/examples.ini");
    std::fs::write(&config, "[example.counter]\nfail_init = true\n").unwrap();
    let initial = defiance_loader::test_host::load_plugins(dir);
    println!("startup: {initial:?}");
    assert!(initial
        .iter()
        .any(|(id, state)| { id == "example.counter" && state.starts_with("Failed") }));
    let poll = || defiance_loader::test_host::poll_startup_retries();
    assert!(poll().is_empty());
    std::fs::write(&config, "[example.counter]\nfail_init = false\n").unwrap();
    let provider = dir.join("../DefianceLoader/plugins/defiance_example_counter.dll");
    let append_overlay = || {
        let mut bytes = std::fs::read(&provider).unwrap();
        bytes.push(0);
        std::fs::write(&provider, bytes).unwrap();
    };
    append_overlay();
    assert!(poll().is_empty());
    assert_eq!(poll(), vec!["example.counter".to_string()]);

    append_overlay();
    defiance_loader::test_host::apply_pending_changes();
    let loaded = defiance_loader::test_host::loaded_plugins();
    let stale_summary = defiance_loader::test_host::plugin_summary();
    println!("loaded after stale apply: {loaded:?}");
    println!("summary after stale apply: {stale_summary}");
    assert!(!loaded.iter().any(|(id, _)| id == "example.counter"));
    assert!(stale_summary.contains("1 failed"));

    assert!(
        poll().is_empty(),
        "the newer stamp starts a fresh stability window"
    );
    assert_eq!(poll(), vec!["example.counter".to_string()]);
    defiance_loader::test_host::apply_pending_changes();
    let loaded = defiance_loader::test_host::loaded_plugins();
    let summary = defiance_loader::test_host::plugin_summary();
    println!("loaded after current apply: {loaded:?}");
    println!("summary after current apply: {summary}");
    assert!(loaded.iter().any(|(id, _)| id == "example.counter"));
    assert!(summary.contains("0 failed"));
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

/// The counter and counter-user at startup; the counter's `fail_init` is
/// set in its config file and it is loaded again with the new value (its init
/// fails, so both stay unloaded), then set back and the counter switched on
/// again (both come back with the value read then).
fn settings(dir: &std::path::Path) {
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    set_value(dir, "example.counter", "fail_init", "true");
    let result = defiance_loader::test_host::resettle_plugin("example.counter");
    println!("resettle: {result:?}");
    print_loaded();
    set_value(dir, "example.counter", "fail_init", "false");
    let result = defiance_loader::test_host::toggle_plugin("example.counter", true);
    println!("on: {result:?}");
    print_loaded();
}

/// A plugin disabled in the startup snapshot is enabled live, then reloaded
/// with a changed setting from the current configuration.
fn live_enable_settings(dir: &std::path::Path) {
    let config = dir.join("../DefianceLoader/config/examples.ini");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[example.counter]\nenabled = false\n").unwrap();
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    print_loaded();

    set_enabled(dir, "example.counter", true);
    let result = defiance_loader::test_host::toggle_plugin("example.counter", true);
    println!("live enable: {result:?}");
    print_loaded();

    set_value(dir, "example.counter", "fail_init", "true");
    let result = defiance_loader::test_host::resettle_plugin("example.counter");
    println!("settings after live enable: {result:?}");
    print_loaded();

    set_value(dir, "example.counter", "fail_init", "false");
    let result = defiance_loader::test_host::toggle_plugin("example.counter", true);
    println!("re-enabled: {result:?}");
    print_loaded();
}

/// Start with the provider disabled and its consumer enabled. Enable both
/// after startup, then reload the provider's DLL; planning must use their
/// current enabled state rather than the startup snapshot.
fn live_enable_reload(dir: &std::path::Path) {
    let config = dir.join("../DefianceLoader/config/examples.ini");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "[example.counter]\nenabled = false\n[example.counter-user]\nenabled = true\n",
    )
    .unwrap();
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    print_loaded();

    set_enabled(dir, "example.counter", true);
    let result = defiance_loader::test_host::toggle_plugin("example.counter", true);
    println!("live enable provider: {result:?}");
    set_enabled(dir, "example.counter-user", true);
    let result = defiance_loader::test_host::toggle_plugin("example.counter-user", true);
    println!("live enable consumer: {result:?}");
    print_loaded();

    let before = defiance_loader::test_host::loaded_plugins();
    let provider = dir.join("../DefianceLoader/plugins/defiance_example_counter.dll");
    let bytes = std::fs::read(&provider).unwrap();
    std::fs::write(&provider, &bytes).unwrap();
    let result = defiance_loader::test_host::reload_plugin("example.counter");
    println!("reload: {result:?}");
    let after = defiance_loader::test_host::loaded_plugins();
    print_owner_changes(&before, &after);
    if let Some(total) = query::<TotalV1>(c"example.counter-user", c"total", 1) {
        println!("total after reload: {}", unsafe { (total.total)() });
    }
}

/// Revoke permission on either the replacement target or a reloadable
/// dependant, then attempt a group reload. Preflight must refuse before any
/// owner is unloaded or service table is replaced.
fn revoke_reload_permission(dir: &std::path::Path, revoke_target: bool) {
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    let provider_before =
        query::<TotalV1>(c"example.counter", c"counter", 1).expect("the provider service");
    let consumer_before =
        query::<TotalV1>(c"example.counter-user", c"total", 1).expect("the consumer service");
    println!("total before: {}", unsafe { (consumer_before.total)() });
    let before = defiance_loader::test_host::loaded_plugins();

    let manifest_name = if revoke_target {
        "defiance_example_counter.plugin.json"
    } else {
        "defiance_example_counter_user.plugin.json"
    };
    let manifest = dir.join("../DefianceLoader/plugins").join(manifest_name);
    let text = std::fs::read_to_string(&manifest).unwrap();
    let changed = if text.contains("\"hot_reload\"") {
        text.replace("\"hot_reload\": true", "\"hot_reload\": false")
    } else {
        let body = text.trim_end().strip_suffix('}').unwrap().trim_end();
        format!(
            "{},\n  \"hot_reload\": false\n}}\n",
            body.trim_end_matches(',')
        )
    };
    assert_ne!(changed, text, "manifest reload permission changed");
    assert!(changed.contains("\"hot_reload\": false"), "{changed}");
    std::fs::write(manifest, changed).unwrap();

    let provider = dir.join("../DefianceLoader/plugins/defiance_example_counter.dll");
    let bytes = std::fs::read(&provider).unwrap();
    std::fs::write(&provider, &bytes).unwrap();
    let result = defiance_loader::test_host::reload_plugin("example.counter");
    println!("reload: {result:?}");
    let after = defiance_loader::test_host::loaded_plugins();
    print_owner_changes(&before, &after);
    print_loaded();

    let provider_after = query::<TotalV1>(c"example.counter", c"counter", 1)
        .expect("the provider service remains available");
    let consumer_after = query::<TotalV1>(c"example.counter-user", c"total", 1)
        .expect("the consumer service remains available");
    println!(
        "provider service unchanged: {}",
        core::ptr::eq(provider_before, provider_after)
    );
    println!(
        "consumer service unchanged: {}",
        core::ptr::eq(consumer_before, consumer_after)
    );
    println!("total after: {}", unsafe { (consumer_after.total)() });
}

fn print_owner_changes(before: &[(String, usize)], after: &[(String, usize)]) {
    for (id, owner) in after {
        let old = before
            .iter()
            .find(|(old_id, _)| old_id == id)
            .map(|(_, old)| *old);
        println!(
            "loaded {id}: {}",
            if old == Some(*owner) { "same" } else { "new" }
        );
    }
}

/// Queue a provider DLL change together with a consumer setting change that
/// makes the consumer's init fail. The failed candidate must not be committed.
fn mixed_reload_config(dir: &std::path::Path) {
    for (id, state) in defiance_loader::test_host::load_plugins(dir) {
        println!("{id}: {state}");
    }
    let before = defiance_loader::test_host::loaded_plugins();
    let provider = dir.join("../DefianceLoader/plugins/defiance_example_counter.dll");
    let mut bytes = std::fs::read(&provider).unwrap();
    // A changed length guarantees the watcher observes a new stamp. The PE
    // loader ignores this trailing overlay byte.
    bytes.push(0);
    std::fs::write(&provider, bytes).unwrap();
    set_value(dir, "example.counter-user", "service_version", "2");

    let result = defiance_loader::test_host::apply_reload_and_config(
        "example.counter",
        "example.counter-user",
    );
    println!("apply mixed: {result:?}");
    let after = defiance_loader::test_host::loaded_plugins();
    print_owner_changes(&before, &after);
    print_loaded();

    let api = defiance_loader::test_host::build_api();
    let version = unsafe {
        let value = (api.config_get)(
            c"example.counter-user".as_ptr(),
            c"service_version".as_ptr(),
        );
        (!value.is_null()).then(|| {
            core::ffi::CStr::from_ptr(value)
                .to_string_lossy()
                .into_owned()
        })
    };
    println!("service version after failed config: {version:?}");
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
