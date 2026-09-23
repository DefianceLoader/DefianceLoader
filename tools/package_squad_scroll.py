"""Build a local scrolling add-on ZIP from the installed game's UI resources.

Reads the game; never writes to it. INIs are created by the loader at launch.
The UI resource itself is deliberately not checked into this repository.

The add-on overlays both unit panels: the infantry panel (weapon, ammunition,
perk and upgrade rows) and the vehicle panel (weapon, ammunition and upgrade
rows). The upgrade columns scroll with a vertical slider in the four-pixel gutter
beside the column.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import zipfile

ROOT = Path(__file__).resolve().parent.parent
PLUGIN = ROOT / 'plugins/squad-management-scroll'
# The game's archives are encrypted and their password is not part of this
# repository: set it in DEFIANCE_PAK_PASSWORD (for example under [env] in the
# untracked mise.local.toml). Unencrypted archives, as in the tests, need none.
PASSWORD_ENV = 'DEFIANCE_PAK_PASSWORD'
RESOURCE = 'scripts/ui/InfantryInfoPanel.txt'
VEHICLE_RESOURCE = 'scripts/ui/VehicleInfoPanel.txt'
# Row geometry of both panels. The upgrade slider runs down the free four-pixel
# gutter right of the upgrade column (x 449-453, the cards' full height), the
# same on both panels. The game's slider lays out and drags along x only; the
# plugin lays out and drags a slider taller than wide along y.
WEAPON_SLIDER = (1, 461, 453, 465)
AMMO_SLIDER = (1, 689, 453, 693)
PERK_SLIDER = (1, 192, 453, 196)
UPGRADE_SLIDER = (449, 49, 453, 417)


def slider(name, rect, direction='horizontal'):
    # Enabled, as the game's own sliders are: a disabled slider ignores clicks
    # and drags, and the stock listener registration skips it, so only the
    # plugin's own wheel handling could move it.
    x0, y0, x1, y1 = rect
    properties = [('tooltip_type', 'simple'), ('enabled', 'true'),
                  ('scaled', 'none'), ('image_type', 'normal'),
                  ('indent_tb', 'indent_top 0 indent_bottom 0'),
                  ('indent_lr', 'indent_left 0 indent_right 0'),
                  ('is_interactable', 'true'),
                  ('client_pic', r'defiance_scroll\track.dds'),
                  ('thumb', r'defiance_scroll\thumb.dds'),
                  ('thumb_click', r'defiance_scroll\thumb_active.dds'),
                  ('direction', direction), ('stretch', 'true')]
    return f'{name}\tslider\t {x0}, {y0}, {x1}, {y1}\t\t\t\t\r\n' + ''.join(
        f'\t\t\t\t\t{key}\t{value}\r\n' for key, value in properties)


def rows_of(data):
    text = data.decode('utf-8-sig')
    if 'df_weapons' in text or 'df_ammo' in text:
        raise ValueError('layout already contains scrolling controls')
    rows = {}
    for line in text.splitlines():
        fields = line.split('\t')
        if len(fields) >= 3 and fields[0]:
            if fields[0] in rows:
                raise ValueError(f'duplicate widget: {fields[0]}')
            rows[fields[0]] = fields
    return text, rows


def check_slots(rows, training):
    # Both supported game DLLs unconditionally dereference these five widgets
    # during stock refresh, even for squads that cannot equip upgrades. The
    # original basis.pak predates them; overlaying it crashes before our hook.
    for i in range(1, 6):
        row = rows.get(f'upgrade_slot_{i}')
        if row is None or row[1] != 'widget':
            raise ValueError(f'current game requires upgrade_slot_{i}; obsolete panel layout')
    # Refuse changed slot geometry rather than placing controls over cards.
    for section in ('weapon', 'ammo'):
        for i in range(6):
            expected = ((1 + i % 3 * 152, 465 + i // 3 * 76,
                         149 + i % 3 * 152, 541 + i // 3 * 76) if section == 'weapon'
                        else (1 + i * 76, 617, 73 + i * 76, 689))
            row = rows.get(f'{section}_slot_{i + 1}')
            if row is None or row[1] != 'widget' or tuple(map(int, row[2].split(','))) != expected:
                raise ValueError(f'unsupported {section} slot layout; overlay not generated')
    if training:
        # The perk pick row (training_slot_*), five cards across the header band.
        for i in range(5):
            expected = (42 + i * 74, 103, 118 + i * 74, 191)
            row = rows.get(f'training_slot_{i + 1}')
            if row is None or row[1] != 'widget' or tuple(map(int, row[2].split(','))) != expected:
                raise ValueError('unsupported training slot layout; overlay not generated')


def layout(data):
    """The infantry panel: weapon, ammunition, perk and upgrade rows."""
    text, rows = rows_of(data)
    check_slots(rows, training=True)
    sliders = (slider('df_weapons', WEAPON_SLIDER) + slider('df_ammo', AMMO_SLIDER)
               + slider('df_upgrades', UPGRADE_SLIDER, 'vertical')
               + slider('df_perks', PERK_SLIDER))
    # These are existing four-pixel gutters: no slot is moved or resized. The
    # perk slider sits in the thin band just under the perk pick row, the
    # upgrade slider in the gutter beside the upgrade column.
    return (text.rstrip('\r\n') + '\r\n' + sliders).encode('utf-8')


def vehicle_layout(data):
    """The vehicle panel: weapon, ammunition and upgrade rows (no perk row)."""
    text, rows = rows_of(data)
    check_slots(rows, training=False)
    sliders = (slider('df_weapons', WEAPON_SLIDER) + slider('df_ammo', AMMO_SLIDER)
               + slider('df_upgrades', UPGRADE_SLIDER, 'vertical'))
    return (text.rstrip('\r\n') + '\r\n' + sliders).encode('utf-8')


def dds(width, height, color):
    """An original, solid RGBA texture in uncompressed DDS format."""
    header = [124, 0x100f, height, width, width * 4, 0, 0] + [0] * 11
    header += [32, 0x41, 0, 32, 0xff0000, 0xff00, 0xff, 0xff000000]
    header += [0x1000, 0, 0, 0, 0]
    assert len(header) == 31
    r, g, b, a = color
    return b'DDS ' + struct.pack('<31I', *header) + bytes([b, g, r, a]) * width * height


def read(archive, name, pak):
    """An archive member, decrypted with the password from the environment."""
    password = os.environ.get(PASSWORD_ENV)
    if archive.getinfo(name).flag_bits & 0x1 and not password:
        raise ValueError(f'{name} in {pak.name} is encrypted; set {PASSWORD_ENV} '
                         '(for example in mise.local.toml) to the game archive password')
    return archive.read(name, pwd=password.encode() if password else None)


def resources(game, resource, make):
    # Archive suffixes describe the update release, not an independent version
    # of this shared UI resource. In particular patch_010_dlc updates the common
    # infantry panel with mandatory upgrade widgets. Never resurrect basis.pak's
    # obsolete panel in the mod's basis override. Use the latest definition in
    # every emitted layer, including compatibility overlays from older packages.
    layers = {'basis'}
    latest = None
    source = None
    paks = [game / 'basis.pak'] + sorted(game.glob('patch_*.pak'))
    if not paks[0].is_file():
        raise ValueError('basis.pak not found in the game directory')
    for pak in paks:
        match = re.search(r'_(dlc\d*)\.pak$', pak.name, re.I)
        layer = match[1].lower() if match else 'basis'
        with zipfile.ZipFile(pak) as archive:
            names = [n for n in archive.namelist() if n.replace('\\', '/').lower() == resource.lower()]
            if len(names) > 1:
                raise ValueError(f'ambiguous UI resource in {pak.name}')
            if names:
                data = read(archive, names[0], pak)
                layers.add(layer)
                latest = data
                source = dict(archive=pak.name, sha256=hashlib.sha256(data).hexdigest())
    if latest is None:
        raise ValueError(f'{resource} not found')
    derived = make(latest)
    return ({layer: derived for layer in sorted(layers)},
            {layer: dict(source, layout_revision=3) for layer in sorted(layers)})


def mod_entries(game):
    """The companion mod tree (game-relative paths -> bytes) and its sources.

    Shared with the loader package so both ZIPs carry the same overlay. Reads
    the installed game's paks; never writes to them.
    """
    prefix = 'mods/defiance_squad_scroll/'
    entries = {}
    entries[prefix + 'mod.json'] = json.dumps(dict(
        name='Defiance squad inventory scrolling',
        description='Independent weapon, ammunition, perk and upgrade scrollbars. Requires the Defiance Loader squad-management-scroll plugin.',
        icon='basis/mod_icon.dds'), indent=2).encode()
    entries[prefix + 'basis/mod_icon.dds'] = dds(160, 90, (83, 118, 127, 255))
    sources = {}
    layers = set()
    for resource, make in ((RESOURCE, layout), (VEHICLE_RESOURCE, vehicle_layout)):
        panel_layers, panel_sources = resources(game, resource, make)
        sources[resource] = panel_sources
        layers.update(panel_layers)
        for layer, data in panel_layers.items():
            entries[prefix + layer + '/' + resource] = data
    for layer in sorted(layers):
        for name, color in [('track', (28, 37, 41, 255)), ('thumb', (124, 175, 187, 255)),
                            ('thumb_active', (183, 224, 229, 255))]:
            entries[prefix + layer + '/textures/ui/pictures/defiance_scroll/' + name + '.dds'] = dds(32, 4, color)
    entries[prefix + 'sources.json'] = json.dumps(sources, indent=2).encode()
    return entries, sources


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=ROOT / 'out/defiance-squad-scroll.zip')
    args = parser.parse_args(argv)
    manifest = PLUGIN / 'defiance_plugin_squad_management_scroll.plugin.json'
    dll = PLUGIN / 'target/release' / json.loads(manifest.read_text())['dll']
    if not dll.is_file():
        parser.error('build the release plugin first')
    entries = {
        'README.md': (PLUGIN / 'README.md').read_bytes(),
        'DefianceLoader/plugins/' + dll.name: dll.read_bytes(),
        'DefianceLoader/plugins/' + manifest.name: manifest.read_bytes(),
    }
    entries.update(mod_entries(args.game)[0])
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(args.out, 'w', zipfile.ZIP_DEFLATED) as archive:
        for name, data in entries.items():
            archive.writestr(name, data)
    sources = json.loads(entries['mods/defiance_squad_scroll/sources.json'])
    print(f'{args.out}\nUI sources: {sources}')


if __name__ == '__main__':
    main()
