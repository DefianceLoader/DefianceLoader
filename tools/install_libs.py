"""Install the Python libraries the tools use (tools/requirements.txt) into the
checkout's own .venv: `mise run setup`, and the last step of `mise run
worktree-init`.

pip runs under the venv's interpreter by path, not the `python` on PATH. When
mise creates .venv during the same invocation (`_.python.venv` in mise.toml
with `create = true`), that invocation's PATH was resolved before the venv
existed, so its `python` is whatever came first on PATH and pip would install
there instead.
"""
import os, pathlib, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main():
    venv = ROOT / ".venv"
    python = venv / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    if not python.exists():
        # mise normally creates it; this covers a run without mise's env.
        subprocess.run([sys.executable, "-m", "venv", str(venv)], check=True)
    subprocess.run(
        [str(python), "-m", "pip", "install", "-r", str(ROOT / "tools" / "requirements.txt")],
        check=True,
    )


if __name__ == "__main__":
    main()
