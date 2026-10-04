"""Exercise the author example DLLs through the production loader startup path."""
import json
import pathlib
import shutil
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
EXAMPLES = ROOT / "examples/services"


def main():
    cases = {
        "normal": ("", {"example.counter": "Active", "example.counter-user": "Active"}),
        "disabled": ("[example.counter]\nenabled=false\n", {"example.counter": "Disabled", "example.counter-user": "Blocked"}),
        "failed": ("[example.counter]\nfail_init=true\n", {"example.counter": "Failed", "example.counter-user": "Blocked"}),
        "version": ("[example.counter-user]\nservice_version=2\n", {"example.counter": "Active", "example.counter-user": "Failed"}),
        "missing": ("", {"example.counter-user": "Blocked"}),
        "undeclared": ("", {"example.counter": "Active", "example.counter-user": "Failed"}),
        "plugin-version": ("", {"example.counter": "Active", "example.counter-user": "Blocked"}),
    }
    for case, (config, expected) in cases.items():
        with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
            root = pathlib.Path(tmp)
            exe = root / "bin/service-host.exe"
            exe.parent.mkdir()
            shutil.copy2(ROOT / "target/release/service-host.exe", exe)
            plugins = root / "DefianceLoader/plugins"
            plugins.mkdir(parents=True)
            for role, dll in [("provider", "defiance_example_counter"), ("consumer", "defiance_example_counter_user")]:
                if case == "missing" and role == "provider":
                    continue
                shutil.copy2(EXAMPLES / "target/release" / (dll + ".dll"), plugins)
                manifest = json.loads((EXAMPLES / role / (dll + ".plugin.json")).read_text())
                if role == "consumer":
                    if case == "undeclared":
                        manifest["depends"] = []
                    if case == "plugin-version":
                        manifest["depends"][0] = {"id": "example.counter", "min": "9.0.0"}
                (plugins / (dll + ".plugin.json")).write_text(json.dumps(manifest))
            cfg = root / "DefianceLoader/config/examples.ini"
            cfg.parent.mkdir()
            cfg.write_text(config)
            result = subprocess.run([str(exe)], check=True, capture_output=True, text=True)
            states = dict(line.split(": ", 1) for line in result.stdout.splitlines() if line.startswith("example."))
            for plugin, state in expected.items():
                assert states.get(plugin, "").startswith(state), (case, states, result.stdout, result.stderr)
            print(f"PASS {case}: {states}", flush=True)
    log_filter_cases()
    reload()
    patch_v1_case()


def log_filter_cases():
    """The real loader gives existing plugin DLLs source-bound log callbacks."""
    for case, include, exclude, written in [
        ("all", "", "", True),
        ("include", " EXAMPLE.COUNTER-USER, ", "", True),
        ("other-only", "example.counter", "", False),
        ("exclude", "", "example.counter-user", False),
        ("both", "example.counter-user", "example.counter-user", False),
    ]:
        with tempfile.TemporaryDirectory(prefix="defiance-log-filter-") as tmp:
            root = pathlib.Path(tmp)
            exe = host_with(root, {"defiance_example_counter": {}, "defiance_example_counter_user": {}})
            cfg = root / "DefianceLoader/config/core.ini"
            cfg.parent.mkdir()
            cfg.write_text(f"[logging]\nlevel = info\ninclude_plugins = {include}\nexclude_plugins = {exclude}\n")
            result = subprocess.run([str(exe)], check=True, capture_output=True, text=True)
            assert "example.counter-user: Active" in result.stdout, (case, result.stdout, result.stderr)
            message = "[info] [example.counter-user] counter-user: shared state verified (two increments)"
            log = (root / "DefianceLoader/logs/defiance-loader.log").read_text()
            assert (message in log) == written, (case, log)
            assert "plugin example.counter-user" in log, (case, log)
            print(f"PASS plugin log filter {case}", flush=True)


def reload():
    """A hot reload of the provider takes the consumer with it: both unload
    and load again under new owners, and the consumer finds the provider's
    service once more. The provider's file is rewritten while it is loaded.
    A consumer that can only load at startup stays instead, and the old
    provider copy it holds stays mapped for it."""
    for fixed in (False, True):
        reload_case(fixed)
    chain_case()
    multiplayer_case()
    add_case()
    remove_case(fixed=False)
    remove_case(fixed=True)
    toggle_case()
    live_enable_settings_case()
    live_enable_reload_case()
    replacement_permission_case("revoke-target")
    replacement_permission_case("revoke-consumer")
    mixed_reload_config_case()
    settings_case()


def host_with(root, plugins_json):
    """A test host in `root` with the example DLLs in `plugins_json` ({dll stem:
    manifest changes}) installed."""
    exe = root / "bin/service-host.exe"
    exe.parent.mkdir()
    shutil.copy2(ROOT / "target/release/service-host.exe", exe)
    plugins = root / "DefianceLoader/plugins"
    plugins.mkdir(parents=True)
    folders = {"defiance_example_counter": "provider", "defiance_example_counter_user": "consumer",
               "defiance_example_counter_watch": "watch"}
    for dll, changes in plugins_json.items():
        shutil.copy2(EXAMPLES / "target/release" / (dll + ".dll"), plugins)
        manifest = json.loads((EXAMPLES / folders[dll] / (dll + ".plugin.json")).read_text())
        manifest.update(changes)
        (plugins / (dll + ".plugin.json")).write_text(json.dumps(manifest))
    return exe


def patch_v1_case():
    """Exercise Core's PatchV1 service on allocated module images, without game DLLs."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-patch-") as tmp:
        exe = pathlib.Path(tmp) / "bin/service-host.exe"
        exe.parent.mkdir()
        shutil.copy2(ROOT / "target/release/service-host.exe", exe)
        result = subprocess.run([str(exe), "patch-v1"], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (lines, result.stderr)
        for expected in [
            "relocated unit: installed at synthetic game.dll+0x180",
            "overlap: refused without changing the existing owner's span",
            "failed unit: later refusal rolled back its earlier write",
            "ownership: spans restore under their installing plugin",
        ]:
            assert expected in lines, (expected, lines, result.stderr)
        print(
            "PASS PatchV1: synthetic module relocation, overlap refusal, rollback, and ownership",
            flush=True,
        )

def chain_case():
    """Counter <- counter-user <- counter-watch (startup-only). Reloading the
    counter keeps old counter-user for the watch, and old counter for old
    counter-user; calls through the watch keep reaching the counter, twice."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        exe = host_with(pathlib.Path(tmp), {
            "defiance_example_counter": {}, "defiance_example_counter_user": {},
            "defiance_example_counter_watch": {"hot_reload": False}})
        result = subprocess.run([str(exe), "chain"], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (result.returncode, lines, result.stderr)
        assert "total before: 2" in lines, lines
        for round in (1, 2):
            assert f"reload {round}: Ok(())" in lines, (lines, result.stderr)
            assert f"total after {round}: 2" in lines, (lines, result.stderr)
        # A freed copy is not always caught by the call: an identical new copy
        # can map at the old address. The first reload must keep both.
        first = next(l for l in lines if l.startswith("retained after 1: "))
        kept = first.split(": ", 1)[1].split(", ")
        assert sorted(kept) == ["example.counter", "example.counter-user"], lines
        print(f"PASS chain: two reloads under a startup-only holder, old copies kept ({first})", flush=True)


def add_case():
    """A plugin dropped into the plugins directory after startup loads, with
    its settings (undeclared at startup) read and written to its group file."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        root = pathlib.Path(tmp)
        exe = host_with(root, {"defiance_example_counter": {}})
        pending = root / "pending"
        pending.mkdir()
        dll = "defiance_example_counter_user"
        shutil.copy2(EXAMPLES / "target/release" / (dll + ".dll"), pending)
        shutil.copy2(EXAMPLES / "consumer" / (dll + ".plugin.json"), pending)
        result = subprocess.run([str(exe), "add"], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (result.returncode, lines, result.stderr)
        assert "add: Ok(())" in lines, (lines, result.stderr)
        assert "loaded: example.counter, example.counter-user" in lines, lines
        assert "total: 2" in lines, lines
        assert "resettle added: Ok(())" in lines, lines
        changed_schema = next(
            (l for l in lines if l.startswith("resettle changed schema: ")), ""
        )
        assert "settings changed; restart the game" in changed_schema, lines
        schema = lines.index(changed_schema)
        assert lines[schema + 1] == "loaded: example.counter, example.counter-user", lines
        config = (root / "DefianceLoader/config/examples.ini").read_text()
        assert "[example.counter-user]" in config and "service_version" in config, config
        print("PASS add: late plugin settings reload against its loaded schema", flush=True)


def remove_case(fixed):
    """A plugin whose files are deleted is unloaded. Its holder unloads with it
    and stays unloaded (its dependency is gone); a startup-only holder instead
    keeps the old copy mapped and keeps working."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        exe = host_with(pathlib.Path(tmp), {
            "defiance_example_counter": {},
            "defiance_example_counter_user": {"hot_reload": False} if fixed else {}})
        result = subprocess.run([str(exe), "remove"], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (result.returncode, lines, result.stderr)
        assert "remove: Ok(())" in lines, (lines, result.stderr)
        if fixed:
            assert "loaded: example.counter-user" in lines, lines
            assert "retained: example.counter" in lines, lines
            print("PASS remove: the removed plugin stays mapped for its startup-only holder", flush=True)
        else:
            assert "loaded: " in lines, lines
            assert "retained: " in lines, lines
            print("PASS remove: the removed plugin and its holder are unloaded", flush=True)


def toggle_case():
    """A plugin switched off in its config file unloads with the plugin that
    needs it; switched on again, both load."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        exe = host_with(pathlib.Path(tmp), {"defiance_example_counter": {}, "defiance_example_counter_user": {}})
        result = subprocess.run([str(exe), "toggle"], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (result.returncode, lines, result.stderr)
        off = lines.index("off: Ok(())")
        assert lines[off + 1] == "loaded: ", lines
        on = lines.index("on: Ok(())")
        assert lines[on + 1] == "loaded: example.counter, example.counter-user", lines
        # a fresh counter, incremented twice by the returning counter-user
        assert "total: 2" in lines, lines
        print("PASS toggle: switched off and on again with the plugin that needs it", flush=True)


def settings_case():
    """A loaded plugin loaded again for changed settings reads the new values:
    `fail_init = true` makes its init fail. Switched on after the value is set
    back, it loads with the value as the file says then."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        exe = host_with(pathlib.Path(tmp), {"defiance_example_counter": {}, "defiance_example_counter_user": {}})
        result = subprocess.run([str(exe), "settings"], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (result.returncode, lines, result.stderr)
        failed = next(i for i, l in enumerate(lines) if l.startswith("resettle: "))
        assert "init returned 1" in lines[failed], lines
        assert lines[failed + 1] == "loaded: ", lines
        on = lines.index("on: Ok(())")
        assert lines[on + 1] == "loaded: example.counter, example.counter-user", lines
        print("PASS settings: loaded again with the values its config file says now", flush=True)


def live_enable_settings_case():
    """A plugin enabled live after startup-disabled settings can reload against
    its current enabled value instead of the startup snapshot."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        exe = host_with(pathlib.Path(tmp), {"defiance_example_counter": {}})
        result = subprocess.run([str(exe), "live-enable-settings"], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (result.returncode, lines, result.stderr)
        assert "live enable: Ok(())" in lines, (lines, result.stderr)
        failed = next(i for i, l in enumerate(lines) if l.startswith("settings after live enable: "))
        assert "init returned 1" in lines[failed], lines
        assert lines[failed + 1] == "loaded: ", lines
        enabled = lines.index("re-enabled: Ok(())")
        assert lines[enabled + 1] == "loaded: example.counter", lines
        print("PASS live enable: settings reload uses the enabled live configuration", flush=True)


def live_enable_reload_case():
    """A startup-disabled provider and its enabled consumer can be enabled
    live, then both reload from their current enabled state."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        root = pathlib.Path(tmp)
        exe = host_with(root, {
            "defiance_example_counter": {},
            "defiance_example_counter_user": {},
        })
        config = root / "DefianceLoader/config/examples.ini"
        config.parent.mkdir(parents=True)
        config.write_text(
            "[example.counter]\nenabled = false\n"
            "[example.counter-user]\nenabled = true\n"
        )
        result = subprocess.run([str(exe), "live-enable-reload"], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (result.returncode, lines, result.stderr)
        assert "live enable provider: Ok(())" in lines, (lines, result.stderr)
        assert "live enable consumer: Ok(())" in lines, (lines, result.stderr)
        assert "reload: Ok(())" in lines, (lines, result.stderr)
        assert "loaded example.counter: new" in lines, lines
        assert "loaded example.counter-user: new" in lines, lines
        assert "total after reload: 2" in lines, lines
        print("PASS live enable: provider and consumer reload after startup enablement", flush=True)


def replacement_permission_case(command):
    """Revoking hot reload on the replacement provider or its reloadable
    consumer refuses the group before owners or service tables change."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        exe = host_with(pathlib.Path(tmp), {
            "defiance_example_counter": {},
            "defiance_example_counter_user": {},
        })
        result = subprocess.run([str(exe), command], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (result.returncode, lines, result.stderr)
        reload_line = next((line for line in lines if line.startswith("reload: ")), "")
        assert "Err(" in reload_line and "restart" in reload_line.lower(), (lines, result.stderr)
        assert "loaded: example.counter, example.counter-user" in lines, lines
        assert "total before: 2" in lines and "total after: 2" in lines, lines
        assert "provider service unchanged: true" in lines, lines
        assert "consumer service unchanged: true" in lines, lines
        assert "loaded example.counter: same" in lines, lines
        assert "loaded example.counter-user: same" in lines, lines
        print(f"PASS {command}: replacement reload permission preserves owners and services", flush=True)


def mixed_reload_config_case():
    """A queued provider DLL reload and consumer settings edit must attempt
    the new setting; a failed init must not publish it as accepted."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        root = pathlib.Path(tmp)
        exe = host_with(root, {
            "defiance_example_counter": {},
            "defiance_example_counter_user": {},
        })
        config = root / "DefianceLoader/config/examples.ini"
        config.parent.mkdir(parents=True)
        config.write_text("[example.counter-user]\nservice_version = 1\n")
        result = subprocess.run([str(exe), "mixed-reload-config"], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (result.returncode, lines, result.stderr)
        outcome = next((line for line in lines if line.startswith("apply mixed: ")), "")
        assert "Err(" in outcome and "example.counter-user" in outcome, (lines, result.stderr)
        assert "loaded example.counter: new" in lines, lines
        assert "loaded: example.counter" in lines, lines
        assert "service version after failed config: Some(\"1\")" in lines, lines
        print("PASS mixed reload/config: failed candidate leaves the accepted consumer settings", flush=True)


def multiplayer_case():
    """A plugin reloaded as no longer multiplayer-safe blocks from then on,
    with the guard installed and nothing blocking at startup."""
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        exe = host_with(pathlib.Path(tmp), {"defiance_example_counter": {}})
        result = subprocess.run([str(exe), "multiplayer"], capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert result.returncode == 0, (result.returncode, lines, result.stderr)
        assert "blockers before: []" in lines, lines
        assert "reload: Ok(())" in lines, (lines, result.stderr)
        assert "blockers after: [example.counter]" in lines, lines
        print("PASS multiplayer: a reload that is no longer safe blocks online play", flush=True)


def reload_case(fixed):
    with tempfile.TemporaryDirectory(prefix="defiance-services-") as tmp:
        root = pathlib.Path(tmp)
        exe = root / "bin/service-host.exe"
        exe.parent.mkdir()
        shutil.copy2(ROOT / "target/release/service-host.exe", exe)
        plugins = root / "DefianceLoader/plugins"
        plugins.mkdir(parents=True)
        for role, dll in [("provider", "defiance_example_counter"), ("consumer", "defiance_example_counter_user")]:
            shutil.copy2(EXAMPLES / "target/release" / (dll + ".dll"), plugins)
            manifest = json.loads((EXAMPLES / role / (dll + ".plugin.json")).read_text())
            if fixed and role == "consumer":
                manifest["hot_reload"] = False
            (plugins / (dll + ".plugin.json")).write_text(json.dumps(manifest))
        result = subprocess.run([str(exe), "reload"], check=True, capture_output=True, text=True)
        lines = result.stdout.splitlines()
        assert "reload: Ok(())" in lines, (lines, result.stderr)
        assert "loaded example.counter: new" in lines, (lines, result.stderr)
        consumer = "same" if fixed else "new"
        assert f"loaded example.counter-user: {consumer}" in lines, (lines, result.stderr)
        if fixed:
            print("PASS reload: provider reloaded; its startup-only consumer stayed", flush=True)
        else:
            print("PASS reload: provider and consumer unloaded and loaded again", flush=True)


if __name__ == "__main__":
    main()
