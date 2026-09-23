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


if __name__ == "__main__":
    main()
