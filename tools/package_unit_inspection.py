"""The unit-inspection companion mod: a greyscale reload bar for the ammo cards.

The stock reload fill (`wpn_reload_bar.dds`) is a teal gradient; a colour
multiplied into it only darkens it. The mod replaces it with a greyscale copy of
the same gradient, same alpha, which the plugin colours by the shown squad's
relation. The copy is 88x8, the stock 88x4, so the plugin can tell which one the
game loaded (a disabled mod leaves the stock texture) and colours only the grey
one. Derived from the installed game's paks; never writes to them.
"""
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import bc7  # noqa: E402
import package_squad_scroll  # noqa: E402  (pak reading, solid DDS)

RESOURCE = "textures/ui/pictures/frames_new/wpn_reload_bar.dds"
MOD_DIR = "defiance_unit_inspection"
MOD_NAME = "Defiance unit inspection colours"
STOCK_SIZE = (88, 4)
# The plugin's `GREY_BAR` in plugins/unit-inspection/src/lib.rs.
SIZE = (88, 8)


def greyscale(data):
    """The grey copy: each pixel's brightest channel, scaled so the brightest
    pixel is white, each stock row twice."""
    width, height, rows = bc7.decode(data)
    if (width, height) != STOCK_SIZE:
        raise ValueError(f"{RESOURCE} is {width}x{height}, not the stock "
                         f"{STOCK_SIZE[0]}x{STOCK_SIZE[1]}; refusing to derive it")
    top = max(max(pixel[:3]) for row in rows for pixel in row)
    if top == 0:
        raise ValueError(f"{RESOURCE} is black")
    grey = []
    for y in range(SIZE[1]):
        row = []
        for r, g, b, a in rows[y * height // SIZE[1]]:
            value = round(max(r, g, b) * 255 / top)
            row.append((value, value, value, a))
        grey.append(row)
    return bc7.encode(grey)


def mod_entries(game):
    """The mod tree (game-relative paths -> bytes) and its sources."""
    prefix = f"mods/{MOD_DIR}/"
    layers, sources = package_squad_scroll.resources(game, RESOURCE, greyscale)
    entries = {
        prefix + "mod.json": json.dumps(dict(
            name=MOD_NAME,
            description="A greyscale reload bar on the ammo cards, which the Defiance Loader "
                        "unit-inspection plugin colours by the shown squad's relation: yours, "
                        "allied, neutral or enemy. Without the plugin the bars stay grey.",
            icon="basis/mod_icon.dds"), indent=2).encode(),
        prefix + "basis/mod_icon.dds": package_squad_scroll.dds(160, 90, (200, 180, 60, 255)),
    }
    for layer, data in layers.items():
        entries[prefix + layer + "/" + RESOURCE] = data
    entries[prefix + "sources.json"] = json.dumps({RESOURCE: sources}, indent=2).encode()
    return entries, sources
