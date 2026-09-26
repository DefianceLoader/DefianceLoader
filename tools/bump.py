"""Show, check and bump the versions of the shipped components.

A component's version lives in several places that must agree: its
`Cargo.toml` (the source of its binary's version info and, for a plugin, the
version it exports), its entry in the lockfile, and the manifest the loader
checks a plugin against, which is the built-in table
(`crates/loader/src/config/builtin.rs`) or a standalone plugin's committed
`.plugin.json`. This tool changes all of them together.

    python tools/bump.py                        list the components and versions
    python tools/bump.py check                  fail if any copy disagrees
    python tools/bump.py patch loader           0.1.0 -> 0.1.1
    python tools/bump.py minor builtins         every built-in gameplay plugin
    python tools/bump.py 1.0.0 core selection   an explicit, higher version

Components: `loader` (the proxy DLL, crash helper and tools), `core`, each
built-in feature by its short name (`selection`, `posture`, ...), and each
standalone plugin (`regroup`, `expanded-ammo-menu`, `squad-management-scroll`,
`unit-inspection`).
Groups: `builtins` (the built-in gameplay plugins, not core) and `all`.
A bump is refused, before anything is written, if it would move a version
backwards or out of a dependant's declared range.
"""
import json
import pathlib
import re
import sys
from dataclasses import dataclass

ROOT = pathlib.Path(__file__).resolve().parent.parent
BUILTIN_TABLE = "crates/loader/src/config/builtin.rs"
VERSION = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
# A literal version a plugin exports instead of its Cargo version.
LITERAL = re.compile(r'b"\d+\.\d+\.\d+\\0"')


@dataclass
class Component:
    name: str
    crate_dir: pathlib.Path
    lockfile: pathlib.Path
    plugin_id: str | None = None      # the id the loader knows it by
    builtin: bool = False             # its manifest is the built-in table
    sidecar: pathlib.Path | None = None


def parse(version):
    match = VERSION.match(version)
    if not match:
        raise SystemExit(f"{version!r} is not a MAJOR.MINOR.PATCH version")
    return tuple(int(part) for part in match.groups())


def crate_version(component):
    text = (component.crate_dir / "Cargo.toml").read_text(encoding="utf-8")
    return re.search(r'\[package\][^\[]*?^version = "([^"]+)"', text, re.S | re.M).group(1)


def crate_name(component):
    text = (component.crate_dir / "Cargo.toml").read_text(encoding="utf-8")
    return re.search(r'\[package\][^\[]*?^name = "([^"]+)"', text, re.S | re.M).group(1)


def builtin_blocks(root):
    """{plugin id: (block start, block end)} in the built-in table's text."""
    text = (root / BUILTIN_TABLE).read_text(encoding="utf-8")
    core = re.search(r'pub const CORE_ID: &str = "([^"]+)";', text).group(1)
    blocks = {}
    for match in re.finditer(r"Builtin \{(.*?)\n    \}", text, re.S):
        found = re.search(r'id: (CORE_ID|"([^"]+)"),', match.group(1))
        blocks[core if found.group(1) == "CORE_ID" else found.group(2)] = match.span(1)
    return text, blocks


def components(root=ROOT):
    out = [Component("loader", root / "crates/loader", root / "Cargo.lock")]
    _, blocks = builtin_blocks(root)
    for plugin_id in blocks:
        short = plugin_id.split(".", 1)[1]
        out.append(Component(short, root / "plugins" / short, root / "Cargo.lock", plugin_id, builtin=True))
    for sidecar in sorted(root.glob("plugins/*/*.plugin.json")):
        crate_dir = sidecar.parent
        plugin_id = json.loads(sidecar.read_text(encoding="utf-8"))["id"]
        out.append(Component(crate_dir.name, crate_dir, crate_dir / "Cargo.lock", plugin_id, sidecar=sidecar))
    return out


def manifest_version(root, component):
    if component.builtin:
        text, blocks = builtin_blocks(root)
        start, end = blocks[component.plugin_id]
        return re.search(r'version: "([^"]+)",', text[start:end]).group(1)
    if component.sidecar:
        return json.loads(component.sidecar.read_text(encoding="utf-8"))["version"]
    return None


def lock_version(component):
    if not component.lockfile.is_file():
        return None
    text = component.lockfile.read_text(encoding="utf-8")
    found = re.search(rf'\[\[package\]\]\nname = "{re.escape(crate_name(component))}"\nversion = "([^"]+)"', text)
    return found.group(1) if found else None


def dependencies(root, all_components):
    """(dependant, dependency id, min, max) for every declared version range."""
    out = []
    for component in all_components:
        if component.sidecar:
            for dependency in json.loads(component.sidecar.read_text(encoding="utf-8")).get("depends", []):
                out.append((component.name, dependency["id"], dependency.get("min"), dependency.get("max")))
    return out


def problems(root=ROOT, versions=None):
    """Every disagreement between a component's copies, every exported literal
    version, and every dependency range the (possibly proposed) versions break."""
    all_components = components(root)
    versions = versions or {}
    found = []
    for component in all_components:
        cargo = crate_version(component)
        for where, value in (("lockfile", lock_version(component)), ("manifest", manifest_version(root, component))):
            if value is not None and value != cargo:
                found.append(f"{component.name}: Cargo.toml says {cargo}, its {where} says {value}")
        lib = component.crate_dir / "src/lib.rs"
        if lib.is_file() and LITERAL.search(lib.read_text(encoding="utf-8")):
            found.append(f"{component.name}: src/lib.rs exports a literal version; use CARGO_PKG_VERSION")
    by_id = {c.plugin_id: versions.get(c.name, crate_version(c)) for c in all_components if c.plugin_id}
    for dependant, dependency, low, high in dependencies(root, all_components):
        version = by_id.get(dependency)
        if version is None:
            continue
        if (low and parse(version) < parse(low)) or (high and parse(version) > parse(high)):
            found.append(f"{dependant} needs {dependency} in [{low or '*'}, {high or '*'}], "
                         f"which would be {version}")
    return found


def select(names, all_components):
    by_name = {c.name: c for c in all_components}
    chosen = []
    for name in names:
        if name == "all":
            chosen += all_components
        elif name == "builtins":
            chosen += [c for c in all_components if c.builtin and c.name != "core"]
        elif name in by_name:
            chosen.append(by_name[name])
        else:
            raise SystemExit(f"unknown component {name!r}; known: {', '.join(by_name)}, builtins, all")
    return list({c.name: c for c in chosen}.values())


def next_version(current, how):
    major, minor, patch = parse(current)
    if how == "major":
        return f"{major + 1}.0.0"
    if how == "minor":
        return f"{major}.{minor + 1}.0"
    if how == "patch":
        return f"{major}.{minor}.{patch + 1}"
    if parse(how) <= parse(current):
        raise SystemExit(f"{how} is not higher than the current {current}")
    return how


def replace_once(path, pattern, version, flags=0):
    text = path.read_text(encoding="utf-8")
    new, count = re.subn(pattern, lambda m: m.group(1) + version + m.group(3), text, count=1, flags=flags)
    if count != 1:
        raise SystemExit(f"{path}: version not found")
    path.write_text(new, encoding="utf-8", newline="\n")


def write(root, component, version):
    replace_once(component.crate_dir / "Cargo.toml", r'(\[package\][^\[]*?^version = ")([^"]+)(")',
                 version, re.S | re.M)
    if lock_version(component) is not None:
        name = re.escape(crate_name(component))
        replace_once(component.lockfile, rf'(\[\[package\]\]\nname = "{name}"\nversion = ")([^"]+)(")', version)
    if component.builtin:
        path = root / BUILTIN_TABLE
        text, blocks = builtin_blocks(root)
        start, end = blocks[component.plugin_id]
        block = re.sub(r'(version: ")([^"]+)(",)', lambda m: m.group(1) + version + m.group(3), text[start:end], count=1)
        path.write_text(text[:start] + block + text[end:], encoding="utf-8", newline="\n")
    if component.sidecar:
        replace_once(component.sidecar, r'(\n  "version": ")([^"]+)(")', version)


def bump(how, names, root=ROOT):
    all_components = components(root)
    chosen = select(names, all_components)
    if not chosen:
        raise SystemExit("name at least one component to bump")
    plan = {c.name: next_version(crate_version(c), how) for c in chosen}
    trouble = problems(root, plan)
    if trouble:
        raise SystemExit("not bumped:\n  " + "\n  ".join(trouble))
    for component in chosen:
        old = crate_version(component)
        write(root, component, plan[component.name])
        print(f"{component.name:<26} {old} -> {plan[component.name]}")
    return plan


def show(root=ROOT):
    for component in components(root):
        where = "built-in table" if component.builtin else (component.sidecar.name if component.sidecar else "-")
        print(f"{component.name:<26} {crate_version(component):<8} {component.plugin_id or '':<34} {where}")


def main(argv):
    if not argv:
        show()
        return 0
    if argv == ["check"]:
        trouble = problems()
        for line in trouble:
            print(line)
        print("versions agree" if not trouble else f"{len(trouble)} problem(s)")
        return 1 if trouble else 0
    if argv[0] in ("-h", "--help", "help"):
        print(__doc__)
        return 0
    bump(argv[0], argv[1:])
    print("rebuild and repackage so the binaries carry the new versions")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
