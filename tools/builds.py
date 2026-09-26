"""The game builds this workspace works with, and where their DLLs are.

Each build's `logic.dll` and `game.dll` live in `bin/<store>/<date>/`, and the
build is called `<store>-<date>`, the name its layout has in `tools/layouts/`
when it has one. The date is the store's release date, or the DLLs' link date
when that is unknown. The DLLs are the game's and never committed, so a build
may be absent from a checkout; `Build.present` says whether it is here.

Every tool and test finds DLLs through this module instead of spelling paths,
so a new build is a new folder (and, to support it, a layout), not an edit to
each tool:

    import builds
    builds.reference().logic                 # the build the patches target
    builds.build("steam-2025-12-23").game
    for b in builds.supported(): ...         # every build Core has a payload for
    builds.RELEASE_COPIES                    # the reference release in each store
    builds.build("steam-2026-09-22").base    # the build it was derived from

    python tools/builds.py                   # list the builds and their state
    python tools/builds.py variants          # assemble and resolve every layout's payload

Each build has a base, the build its layout and sites are derived from, so a
new build is compared with its neighbour rather than with the reference: a GOG
build with the previous GOG build, a Steam build with the GOG build released
beside it (the stores ship each update in parallel). A layout names its base
("base"), so adding a build never changes another's lineage;
`proposed_base` applies the rule for a build that has no layout yet. The
reference has none, and its Steam copy's is the reference.

The two Rust tests that read a DLL (`crates/loader/src/trace.rs`,
`injector/src/main.rs`'s default) name the same paths directly.
"""
import dataclasses, hashlib, json, pathlib, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
BIN = ROOT / "bin"
LAYOUTS = ROOT / "tools" / "layouts"

# The GOG release every patch was written against, and its Steam copy (the same
# release, other bytes), which the reference's signatures resolve directly.
REFERENCE = "gog-2025-12-23"
RELEASE_COPIES = ("gog-2025-12-23", "steam-2025-12-23")


@dataclasses.dataclass(frozen=True)
class Build:
    name: str

    @property
    def store(self) -> str:
        return self.name.split("-", 1)[0]

    @property
    def date(self) -> str:
        return self.name.split("-", 1)[1]

    @property
    def folder(self) -> pathlib.Path:
        return BIN / self.store / self.date

    @property
    def logic(self) -> pathlib.Path:
        return self.folder / "logic.dll"

    @property
    def game(self) -> pathlib.Path:
        return self.folder / "game.dll"

    @property
    def present(self) -> bool:
        return self.logic.is_file() and self.game.is_file()

    @property
    def layout(self) -> pathlib.Path | None:
        path = LAYOUTS / f"{self.name}.json"
        return path if path.is_file() else None

    def require(self) -> "Build":
        """This build, or exit naming the missing DLLs."""
        missing = [relative(p) for p in (self.logic, self.game) if not p.is_file()]
        if missing:
            raise SystemExit(f"build {self.name}: {', '.join(missing)} missing (see docs/development.md)")
        return self

    @property
    def base(self) -> "Build | None":
        """The build this one is derived from (see the module's docs)."""
        if self.name == REFERENCE:
            return None
        if self.name in RELEASE_COPIES:
            return reference()
        if self.layout is not None:
            named = json.loads(self.layout.read_text(encoding="utf-8")).get("base")
            if named:
                return build(named)
        return proposed_base(self.name)

    def lineage(self) -> list["Build"]:
        """The builds from the reference to this one, each the next one's base."""
        chain = [self]
        while chain[-1].base is not None:
            if chain[-1].base in chain:
                raise SystemExit(f"build {self.name}: its bases form a loop")
            chain.append(chain[-1].base)
        return chain[::-1]

    def hashes_match_layout(self) -> bool | None:
        """Whether the DLLs are the ones the layout names; None without either."""
        if not self.present or self.layout is None:
            return None
        profile = json.loads(self.layout.read_text(encoding="utf-8"))
        digest = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
        return (digest(self.logic), digest(self.game)) == (profile["logic_sha256"], profile["game_sha256"])


def relative(path: pathlib.Path) -> str:
    """`path` relative to the repository, with forward slashes."""
    return path.relative_to(ROOT).as_posix()


def build(name: str) -> Build:
    """The build called `name` (`<store>-<date>`), present or not."""
    store, _, date = name.partition("-")
    if store not in ("gog", "steam") or len(date) != 10:
        raise ValueError(f"not a build name: {name!r} (expected <store>-<YYYY-MM-DD>)")
    return Build(name)


def reference() -> Build:
    return build(REFERENCE)


def proposed_base(name: str) -> Build:
    """The base the rule gives a new build: for GOG, the latest supported GOG
    build before it; for Steam, the latest supported GOG build no later than
    it, the one the same update shipped as."""
    new = build(name)
    candidates = [b for b in supported() if b.store == "gog" and b.name != name
                  and (b.date < new.date if new.store == "gog" else b.date <= new.date)]
    return max(candidates, key=lambda b: b.date) if candidates else reference()


def on_disk() -> list[Build]:
    """Every build with a folder under bin/, oldest first."""
    found = [Build(f"{store.name}-{folder.name}")
             for store in sorted(BIN.glob("*")) if store.is_dir()
             for folder in sorted(store.glob("*")) if folder.is_dir()]
    return sorted(found, key=lambda b: (b.date, b.store))


def with_layouts() -> list[Build]:
    """Every build tools/layouts has a profile for, present or not."""
    return sorted((Build(p.stem) for p in LAYOUTS.glob("*.json")), key=lambda b: (b.date, b.store))


def supported() -> list[Build]:
    """The builds Core has a payload for: the reference release in both stores
    and every build with a layout."""
    return [build(n) for n in RELEASE_COPIES] + with_layouts()


def variants() -> int:
    """Assemble and resolve every layout's payload (the `variants` task)."""
    python = sys.executable
    # A base's variant is resolved before the builds derived from it.
    for b in sorted(with_layouts(), key=lambda b: len(b.lineage())):
        b.require()
        for command in ([python, "tools/payload.py", "--layout", b.name],
                        [python, "tools/icon.py", "--layout", b.name],
                        [python, "tools/variant.py", str(b.layout), str(b.logic), str(b.game)]):
            print(f"$ {' '.join(str(c) for c in command[1:])}", flush=True)
            if subprocess.call(command, cwd=ROOT) != 0:
                return 1
    return 0


def main(argv):
    if argv[1:] == ["variants"]:
        return variants()
    if argv[1:]:
        raise SystemExit(__doc__)
    names = {b.name for b in on_disk()} | {b.name for b in supported()}
    for b in sorted((build(n) for n in names), key=lambda b: (b.date, b.store)):
        match = b.hashes_match_layout()
        state = ("present" if b.present else "absent") + (
            "" if match is None else (", layout hashes match" if match else ", LAYOUT HASHES DIFFER"))
        role = "reference" if b.name == REFERENCE else (
            "supported" if b in supported() else "no payload")
        base = f"  base {b.base.name}" if b.base else ""
        print(f"{b.name:18} {role:10} {state}{base}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
