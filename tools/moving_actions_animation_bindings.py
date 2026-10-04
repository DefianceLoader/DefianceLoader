"""Derive per-build native bindings for moving-actions-animation.

Run with the repository Python environment after all supported game DLLs are
present. The Steam 2026-09-25 functions are the original semantic anchors;
their masked instruction patterns are resolved uniquely in every supported
logic.dll. Vtables are independently recovered from MSVC RTTI.
"""
import hashlib
import re
import struct

import builds
from pe import Image
from rtti import Rtti
from regroup_bindings import pattern


ANCHORS = {
    "update": 0x2C3990,
    "position": 0x436930,
    "rotation": 0x436B30,
    "slerp": 0x136910,
}
CLASSES = {
    "human_animation_vt": ".?AVHumanAnimationFacet@Leonardo@@",
    "human_chassis_vt": ".?AVHumanChassisFacet@Leonardo@@",
}


def derive(build):
    image = Image(str(build.require().logic))
    anchor = Image(str(builds.build("steam-2026-09-25").require().logic))
    text = next(s for s in image.sections if s[0] == ".text")
    blob = image.read(text[1], text[2])
    found = {}
    for name, rva in ANCHORS.items():
        f = anchor.function_of(rva)
        if not f or f[0] != rva:
            raise RuntimeError(f"anchor {name} is not a function entry")
        size = min(256, f[1] - rva)
        hits = list(re.finditer(pattern(anchor, rva, size), blob, re.DOTALL))
        if len(hits) != 1:
            raise RuntimeError(f"{build.name}: {name} signature matched {len(hits)} locations")
        at = hits[0].start() + text[1]
        found[name] = at

    rtti = Rtti(image)
    for key, name in CLASSES.items():
        matches = [vt for mangled, _, cols in rtti.find(name) if mangled == name
                   for _, vts in cols for vt in vts]
        if len(matches) != 1:
            raise RuntimeError(f"{build.name}: {name} resolved to {matches}")
        found[key] = matches[0]
    methods = rtti.methods(found["human_animation_vt"])
    if len(methods) <= 10:
        raise RuntimeError(f"{build.name}: HumanAnimationFacet vtable has no update slot 10")
    found["update_vfunc10"] = methods[10]
    entry = image.read(found["update_vfunc10"], 5)
    if len(entry) != 5 or entry[0] != 0xE9:
        raise RuntimeError(f"{build.name}: update vfunc slot 10 is not a near-jump thunk")
    target = found["update_vfunc10"] + 5 + struct.unpack("<i", entry[1:])[0]
    if target != found["update"]:
        raise RuntimeError(
            f"{build.name}: update vfunc slot 10 reaches {target:#x}, not update {found['update']:#x}")
    return image, found


def main():
    for build in builds.supported():
        try:
            image, found = derive(build)
        except Exception as error:
            if build.date < "2026-01-01":
                print(f"{build.name}: unsupported legacy layout; {error}")
            else:
                raise
            continue
        digest = hashlib.sha256(build.logic.read_bytes()).hexdigest()
        checks = {key: image.read(value, 16).hex(" ") for key, value in found.items()
                  if key in ANCHORS}
        print(build.name, digest)
        print("  " + " ".join(f"{k}=0x{v:x}" for k, v in found.items()))
        for name, raw in checks.items():
            print(f"  {name}: {raw}")


if __name__ == "__main__":
    main()
