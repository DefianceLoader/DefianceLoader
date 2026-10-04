"""Package the experimental plugin and localized primary-pickup tooltip rows.

Locale tables are derived from the installed game, kept out of the repository,
and overlaid by a separate companion mod. Stock archives are never modified.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import xml.etree.ElementTree as ET
from xml.dom import minidom
import zipfile

from package_squad_scroll import dds, read
from sheets_xml import SS, read_rows

ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / 'plugins/weapon-drops'
MOD = 'defiance_weapon_drops'


def table(data):
    root = ET.fromstring(data)
    tables = root.findall('.//' + SS + 'Table')
    if len(tables) != 1:
        raise ValueError('expected one SpreadsheetML table')
    rows = read_rows(tables[0])
    if not rows:
        raise ValueError('empty SpreadsheetML table')
    header = {value[0].strip(): column for column, value in rows[0][1].items()}
    records = [{name: cells.get(column, ('', None, None))[0]
                for name, column in header.items()} for _, cells in rows[1:]]
    return root, tables[0], header, records


def pickup_locale(guns, inventory, weapons):
    """Fill missing name/description/hint rows without replacing existing keys."""
    root, sheet, header, records = table(guns)
    if set(header) != {'sysname', 'content'}:
        raise ValueError('unsupported gun locale columns')
    _, _, _, item_records = table(inventory)
    items = {row['sysname']: row for row in item_records}
    existing = {row['sysname']: row['content'] for row in records}
    hint = existing.get('inf_m72law_add') or existing.get('inf_minimi_add')
    if not hint:
        raise ValueError('stock localized pickup hint not found')
    additions = {}
    for item, gun in sorted(weapons.items()):
        localized = items.get(item)
        if not localized or not localized.get('content'):
            continue
        values = {gun: localized['content'],
                  gun + '_desc': localized.get('description', ''),
                  gun + '_add': hint}
        for key, content in values.items():
            if key not in existing and key not in additions:
                additions[key] = content
    # The native table reader compares qualified tag/attribute names. Preserve
    # the authored default namespace and ss: attributes instead of introducing
    # ElementTree's ns0: element names, which that reader cannot find.
    document = minidom.parseString(guns)
    namespace = SS[1:-1]
    dom_table = document.getElementsByTagNameNS(namespace, 'Table')[0]
    prefix = (dom_table.prefix + ':') if dom_table.prefix else ''
    attribute_prefix = next((attribute.localName for attribute in
                             document.documentElement.attributes.values()
                             if attribute.prefix == 'xmlns' and attribute.value == namespace), None)
    if attribute_prefix is None:
        attribute_prefix = 'ss'
        document.documentElement.setAttribute('xmlns:ss', namespace)
    def element(name):
        return document.createElementNS(namespace, prefix + name)
    def attribute(node, name, value):
        node.setAttributeNS(namespace, attribute_prefix + ':' + name, value)
    for key, content in additions.items():
        row = element('Row')
        dom_table.appendChild(row)
        for name, column in sorted(header.items(), key=lambda pair: pair[1]):
            cell = element('Cell')
            attribute(cell, 'Index', str(column))
            row.appendChild(cell)
            data = element('Data')
            attribute(data, 'Type', 'String')
            data.appendChild(document.createTextNode(key if name == 'sysname' else content))
            cell.appendChild(data)
    if SS + 'ExpandedRowCount' in sheet.attrib:
        # Appended rows follow the last physical row, including empty ones.
        rows = sheet.findall(SS + 'Row')
        count = 0
        for row in rows:
            count = int(row.get(SS + 'Index', count + 1))
        attribute(dom_table, 'ExpandedRowCount', str(count + len(additions)))
    return document.toxml(encoding='utf-8'), len(additions)


def latest(paks, resource, base_layer):
    data = None
    layers = {base_layer}
    sources = []
    for pak in paks:
        with zipfile.ZipFile(pak) as archive:
            matches = [name for name in archive.namelist()
                       if name.replace('\\', '/').lower() == resource.lower()]
            if len(matches) > 1:
                raise ValueError(f'ambiguous {resource} in {pak.name}')
            if matches:
                data = read(archive, matches[0], pak)
                match = re.search(r'_(dlc\d*)\.pak$', pak.name, re.I)
                layers.add(match[1].lower() if match else base_layer)
                sources.append({'archive': pak.name,
                                'sha256': hashlib.sha256(data).hexdigest()})
    if data is None:
        raise ValueError(f'{resource} not found')
    return data, layers, sources


def mod_entries(game):
    game = Path(game)
    scripts, _, script_sources = latest(
        [game / 'basis.pak'] + sorted(game.glob('patch_*.pak')),
        'scripts/species/inventory_items.xml', 'basis')
    _, _, _, records = table(scripts)
    weapons = {row['sysname']: default_gun(row['squad_gunmount']) for row in records
               if row.get('item_type', '').strip().lower() == 'weapon'
               and row.get('squad_gunmount', '').strip()
               and any(slot.strip().lower().startswith(('rifles', 'pistols', 'shotguns'))
                       for slot in row.get('slot_type', '').split(','))}
    if not weapons:
        raise ValueError('primary inventory items not found')
    prefix = f'mods/{MOD}/'
    entries = {prefix + 'mod.json': json.dumps({
        'name': 'Defiance primary weapon pickup labels',
        'description': 'Localized ground-pickup tooltips for primary weapons. '
                       'Primary death drops require the optional weapon-drops plugin.',
        'icon': 'basis/mod_icon.dds'}, indent=2).encode(),
        prefix + 'basis/mod_icon.dds': dds(160, 90, (83, 118, 127, 255))}
    sources = {'inventory': script_sources, 'locales': {}}
    for directory in sorted((game / 'localization').iterdir()):
        if not directory.is_dir():
            continue
        base = directory / f'basis_{directory.name}.pak'
        if not base.is_file():
            continue
        paks = [base] + sorted(directory.glob('patch_*.pak'))
        guns, gun_layers, gun_sources = latest(paks, 'locale/guns.xml', base.stem)
        items, _, item_sources = latest(paks, 'locale/inventory_items.xml', base.stem)
        derived, count = pickup_locale(guns, items, weapons)
        for layer in sorted(gun_layers):
            entries[prefix + f'localization/{directory.name}/{layer}/locale/guns.xml'] = derived
        sources['locales'][directory.name] = {
            'guns': gun_sources, 'items': item_sources, 'added_rows': count}
    if not sources['locales']:
        raise ValueError('installed localization archives not found')
    entries[prefix + 'sources.json'] = json.dumps(sources, indent=2).encode()
    return entries, sources


def default_gun(value):
    """Inventory mounts contain a default and optional squad:gun overrides."""
    defaults = [entry.strip() for entry in value.split(',')
                if entry.strip() and ':' not in entry]
    if len(defaults) != 1:
        raise ValueError('expected one default inventory gun')
    return defaults[0]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--game', type=Path, default=os.environ.get('DEFIANCE_GAME_DIR'))
    parser.add_argument('--out', type=Path, default=ROOT / 'out/defiance-weapon-drops.zip')
    args = parser.parse_args(argv)
    if args.game is None:
        parser.error('set DEFIANCE_GAME_DIR or pass --game')
    import stage
    game = stage.game_root(stage.bin_directory(args.game))
    if game is None:
        parser.error('game root with basis.pak not found')
    manifest = PLUGIN / 'defiance_plugin_weapon_drops.plugin.json'
    dll = PLUGIN / 'target/release/defiance_plugin_weapon_drops.dll'
    if not dll.is_file():
        parser.error('build the release plugin first')
    entries, sources = mod_entries(game)
    entries.update({'README.md': (PLUGIN / 'README.md').read_bytes(),
                    'DefianceLoader/plugins/' + dll.name: dll.read_bytes(),
                    'DefianceLoader/plugins/' + manifest.name: manifest.read_bytes()})
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(args.out, 'w', zipfile.ZIP_DEFLATED) as archive:
        for name, data in entries.items():
            archive.writestr(name, data)
    print('Built', args.out.name)
    print('Added locale rows:', {language: data['added_rows']
                                 for language, data in sources['locales'].items()})


if __name__ == '__main__':
    main()
