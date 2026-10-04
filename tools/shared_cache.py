"""The cache directory every worktree of this repository shares.

Results that depend only on file contents (an analysis keyed by the DLLs'
sha256, a test's recorded passes keyed by its inputs' digest) are as valid in
one worktree as in another, so they live outside any single worktree: in the
main checkout's out/cache/, found through git's common directory.
DEFIANCE_CACHE_DIR overrides it; outside a git checkout it is this checkout's
out/cache/.

Writers replace whole files (`write_text`), so a reader in another worktree
never sees half a file; two writers racing lose one result, which is then
recomputed.
"""
import functools
import os
import pathlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent


@functools.cache
def directory():
    override = os.environ.get("DEFIANCE_CACHE_DIR")
    if override:
        path = pathlib.Path(override)
    else:
        try:
            common = subprocess.run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
                                    cwd=ROOT, check=True, capture_output=True, text=True).stdout.strip()
            path = pathlib.Path(common).parent / "out" / "cache"
        except (OSError, subprocess.CalledProcessError):
            path = ROOT / "out" / "cache"
    path.mkdir(parents=True, exist_ok=True)
    return path


def write_text(path, text):
    """Write `path` whole: a temporary file in the same folder, then a rename."""
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
        os.replace(temp, path)
    except OSError:
        # Another process holds the target open (Windows refuses the rename);
        # the result is only lost, not corrupted.
        pathlib.Path(temp).unlink(missing_ok=True)
