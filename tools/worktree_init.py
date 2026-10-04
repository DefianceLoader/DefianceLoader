"""Give a new git worktree the untracked files the main checkout has: every
path `.worktreeinclude` lists (the game DLLs in bin/, mise.local.toml, the
assembly stamps in out/stamps/) is
copied from the main worktree unless the worktree already has it. Lines are
literal paths relative to the repository root; `#` starts a comment. When the
main checkout has a graft graph and graft is installed, it also builds the
worktree's own graph.

Run from inside the worktree: `mise run worktree-init`, which then installs
the Python libraries into the worktree's own .venv.
"""
import pathlib, shutil, subprocess


def git(*args):
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout.strip()


def main():
    here = pathlib.Path(git("rev-parse", "--show-toplevel"))
    main_root = pathlib.Path(git("rev-parse", "--path-format=absolute", "--git-common-dir")).parent
    if here.resolve() == main_root.resolve():
        print("this is the main worktree; nothing to copy")
        return
    listing = main_root / ".worktreeinclude"
    if not listing.exists():
        print(f"{listing} does not exist; nothing to copy")
        return
    for line in listing.read_text().splitlines():
        entry = line.split("#", 1)[0].strip().strip("/")
        if not entry:
            continue
        src, dst = main_root / entry, here / entry
        if dst.exists():
            print(f"have   {entry}")
        elif not src.exists():
            print(f"absent {entry} (not in the main worktree either)")
        else:
            dst.parent.mkdir(parents=True, exist_ok=True)
            if src.is_dir():
                shutil.copytree(src, dst)
            else:
                shutil.copy2(src, dst)
            print(f"copied {entry}")
    # Without its own graft/, graft finds the main checkout's graph in a parent
    # directory and answers with that branch's files and lines.
    # graft/ carries its own ignore files (listed above), so the build must not
    # add root .gitignore and .ignore entries.
    wiring = pathlib.Path("graft", ".graph", "wiring.json")
    graft = shutil.which("graft")  # graft.cmd on Windows, which needs its full name
    if graft and (main_root / wiring).exists() and not (here / wiring).exists():
        subprocess.run([graft, "build", ".", "--no-gitignore", "--no-ignore"], cwd=here, check=True)


if __name__ == "__main__":
    main()
