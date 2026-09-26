"""Build ahead of time: what the test tasks will build, run on every save burst.

`mise run watch` runs this under watchexec, which restarts it when files change
again. It runs exactly the builds the test tasks run (same profiles, features
and target directories), so when a test task builds next, cargo
finds everything up to date. Results go where a person can read
them without rebuilding:

    out/watch/status.json   {"state": "running"|"finished", "exit": N,
                             "steps": [{"step", "exit", "seconds"}], ...}
    out/watch/last.log      the full output of the latest run

Wait for "finished" (the file is replaced atomically), then read "exit" and,
on failure, last.log. `python tools/watch.py` runs one pass by hand.
"""
import datetime
import json
import os
import pathlib
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "out" / "watch"
PARITY = ("defiance-plugin-core/parity-test,defiance-plugin-firing/parity-test,"
          "defiance-plugin-selection/parity-test,defiance-plugin-pickup/parity-test")
STANDALONE = ["plugins/regroup", "plugins/expanded-ammo-menu", "plugins/squad-management-scroll",
              "plugins/unit-inspection"]
STEPS = [
    ("assemble", [sys.executable, "tools/stamp.py", "assemble"]),
    ("build", ["cargo", "build", "--release"]),
    ("parity build", ["cargo", "build", "--release", "--target-dir", "out/controls-parity",
                      "-p", "defiance-plugin-core", "-p", "defiance-plugin-firing",
                      "-p", "defiance-plugin-selection", "-p", "defiance-plugin-pickup",
                      "--features", PARITY]),
    ("crash-test build", ["cargo", "build", "--release", "--target-dir", "out/crash-tests",
                          "-p", "defiance-loader", "--features", "test-host"]),
    ("crash plugin build", ["cargo", "build", "--release", "--target-dir", "out/crash-tests",
                            "--manifest-path", "tools/crash-plugin/Cargo.toml"]),
    ("workspace tests build", ["cargo", "test", "--workspace", "--no-run"]),
    *[(f"{p.split('/')[1]} build", ["cargo", "build", "--release", "--manifest-path", f"{p}/Cargo.toml"])
      for p in STANDALONE],
    *[(f"{p.split('/')[1]} tests build", ["cargo", "test", "--no-run", "--manifest-path", f"{p}/Cargo.toml"])
      for p in STANDALONE],
]


def publish(status):
    OUT.mkdir(parents=True, exist_ok=True)
    temporary = OUT / "status.json.tmp"
    temporary.write_text(json.dumps(status, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, OUT / "status.json")


def main():
    now = lambda: datetime.datetime.now().isoformat(timespec="seconds")
    status = {"state": "running", "started": now(), "steps": []}
    publish(status)
    worst = 0
    with open(OUT / "last.log", "w", encoding="utf-8", errors="replace") as log:
        for name, command in STEPS:
            started = time.monotonic()
            log.write(f"\n=== {name}: {' '.join(command)}\n")
            log.flush()
            result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
            status["steps"].append({"step": name, "exit": result.returncode,
                                    "seconds": round(time.monotonic() - started, 1)})
            worst = worst or result.returncode
            publish(status)
    status.update(state="finished", finished=now(), exit=worst)
    publish(status)
    failed = [s["step"] for s in status["steps"] if s["exit"]]
    print(f"[watch] {status['finished']} " + ("all builds up to date" if not failed
                                              else f"FAILED: {', '.join(failed)} (out/watch/last.log)"))
    return worst


if __name__ == "__main__":
    sys.exit(main())
