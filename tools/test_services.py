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
    reload()


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
        config = (root / "DefianceLoader/config/examples.ini").read_text()
        assert "[example.counter-user]" in config and "service_version" in config, config
        print("PASS add: a plugin added while running loads with its settings", flush=True)


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
