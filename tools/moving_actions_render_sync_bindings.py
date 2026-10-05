"""Resolve the render-sync functions and RTTI tables for each 2026 build."""
from dataclasses import dataclass

import builds
from moving_grenades_bindings import Mapper
from pe import Image


REFERENCE = "steam-2026-09-25"
SUPPORTED = (
    "steam-2026-09-25",
    "steam-2026-09-22",
    "gog-2026-09-25",
    "gog-2026-09-14",
)
FUNCTIONS = {
    "shot": 0x29AE10,
    "primary_shot": 0x29A690,
    "gunner_tick": 0x2D7B00,
    "client_tick": 0x2DCA80,
    "rebind": 0x4327B0,
    "handoff": 0x2D8590,
}
TABLES = {
    "animation_vt": 0x72C118,
    "gunner_vt": 0x72CD00,
    "gunner_client_vt": 0x72CED0,
}
WORLD_ATTACH = 0x154D40
WORLD_DETACH = 0x1551A0


@dataclass(frozen=True)
class Profile:
    name: str
    logic: str
    world: str
    bindings: dict[str, int]


def profiles():
    reference = Image(str(builds.build(REFERENCE).require().logic))
    rows = []
    for name in SUPPORTED:
        build = builds.build(name).require()
        image = Image(str(build.logic))
        mapper = Mapper(reference, image)
        resolved = {key: mapper.code(rva) for key, rva in FUNCTIONS.items()}
        resolved.update({key: mapper.table(rva) for key, rva in TABLES.items()})
        rows.append(Profile(
            name=name,
            logic=str(build.logic),
            world=str(build.logic.parent / "world2.dll") if (build.logic.parent / "world2.dll").exists()
            else str(builds.build("gog-2026-09-25").logic.parent / "world2.dll"),
            bindings=resolved,
        ))
    return rows


if __name__ == "__main__":
    for profile in profiles():
        print(profile.name, profile.bindings)
