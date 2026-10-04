"""Check derived pickup locale rows and companion-mod archive layering."""
import tempfile
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET
import zipfile

from package_weapon_drops import MOD, mod_entries, pickup_locale, table
from sheets_xml import SS


def workbook(headers, rows):
    root = ET.Element(SS + 'Workbook')
    sheet = ET.SubElement(ET.SubElement(root, SS + 'Worksheet'), SS + 'Table',
                          {SS + 'ExpandedRowCount': str(len(rows) + 1)})
    for values in [headers, *rows]:
        row = ET.SubElement(sheet, SS + 'Row')
        for value in values:
            cell = ET.SubElement(row, SS + 'Cell')
            ET.SubElement(cell, SS + 'Data', {SS + 'Type': 'String'}).text = value
    return ET.tostring(root, encoding='utf-8')


class PickupLocaleTests(unittest.TestCase):
    def test_native_qualified_element_and_attribute_names_are_preserved(self):
        # Stock tables use unprefixed element names and literal ss: attributes.
        # A namespace-aware Python parse alone cannot catch an ns0: rewrite.
        guns = b'''<?xml version="1.0"?><?mso-application progid="Excel.Sheet"?>
<Workbook xmlns="urn:schemas-microsoft-com:office:spreadsheet"
 xmlns:ss="urn:schemas-microsoft-com:office:spreadsheet">
<Worksheet ss:Name="guns"><Table ss:ExpandedRowCount="4">
<Row><Cell><Data ss:Type="String">sysname</Data></Cell><Cell><Data ss:Type="String">content</Data></Cell></Row>
<Row><Cell><Data ss:Type="String">inf_m72law_add</Data></Cell><Cell><Data ss:Type="String">Hint</Data></Cell></Row>
<Row/><Row/>
</Table></Worksheet></Workbook>'''
        inventory = workbook(['sysname', 'content', 'description'], [['rifle_item', 'Name', 'Description']])
        data, count = pickup_locale(guns, inventory, {'rifle_item': 'rifle'})
        self.assertEqual(count, 3)
        from xml.dom import minidom
        document = minidom.parseString(data)
        self.assertEqual(document.documentElement.tagName, 'Workbook')
        self.assertEqual(len(document.getElementsByTagName('Table')), 1)
        self.assertEqual(len(document.getElementsByTagName('Row')), 7)
        self.assertEqual(document.getElementsByTagName('Table')[0].getAttribute('ss:ExpandedRowCount'), '7')
        self.assertTrue(all(node.getAttribute('ss:Type') == 'String' for node in document.getElementsByTagName('Data')))
        self.assertIn(b'<?mso-application progid="Excel.Sheet"?>', data)
        self.assertNotIn(b'<ns0:', data)

    def test_aliases_reuse_localized_item_text_and_preserve_existing_rows(self):
        guns = workbook(['sysname', 'content'], [
            ['inf_m72law_add', 'Collecter cette arme'], ['existing', 'Unchanged'],
            ['rifle_desc', 'Existing gun description']])
        inventory = workbook(['sysname', 'content', 'description', 'short_description'],
                             [['rifle_item', 'Fusil amélioré', 'Line 1\n\nLine 2', ''],
                              ['rifle', 'Rifle slot label', '', '']])
        data, count = pickup_locale(guns, inventory, {'rifle_item': 'rifle'})
        _, sheet, _, rows = table(data)
        contents = {row['sysname']: row['content'] for row in rows}
        self.assertEqual(count, 2)
        self.assertEqual(contents['rifle'], 'Fusil amélioré')
        self.assertEqual(contents['rifle_desc'], 'Existing gun description')
        self.assertEqual(contents['rifle_add'], 'Collecter cette arme')
        self.assertEqual(contents['existing'], 'Unchanged')
        self.assertEqual(int(sheet.get(SS + 'ExpandedRowCount')), len(rows) + 1)
        repeated, count = pickup_locale(data, inventory, {'rifle_item': 'rifle'})
        self.assertEqual(count, 0)
        self.assertEqual(table(repeated)[3], rows)
        fresh = workbook(['sysname', 'content'], [['inf_m72law_add', 'Hint']])
        fresh, count = pickup_locale(fresh, inventory, {'rifle_item': 'rifle'})
        self.assertEqual(count, 3)
        self.assertEqual({row['sysname']: row['content'] for row in table(fresh)[3]}[
            'rifle_desc'], 'Line 1\n\nLine 2')

    def test_unsupported_locale_or_missing_hint_fails(self):
        inventory = workbook(['sysname', 'content'], [])
        with self.assertRaises(ValueError):
            pickup_locale(workbook(['sysname', 'description'], []), inventory, {})
        with self.assertRaises(ValueError):
            pickup_locale(workbook(['sysname', 'content'], []), inventory, {})

    def test_companion_uses_latest_locale_in_every_gun_layer(self):
        with tempfile.TemporaryDirectory() as directory:
            game = Path(directory)
            scripts = workbook(['sysname', 'item_type', 'slot_type', 'squad_gunmount'],
                               [['rifle_item', 'weapon', 'rifles', 'rifle, Enemy: rifle_enemy'],
                                ['shotgun_item', 'weapon', 'shotguns', 'shotgun'],
                                ['rocket_item', 'weapon', 'rpg', 'rocket']])
            with zipfile.ZipFile(game / 'basis.pak', 'w') as archive:
                archive.writestr('scripts/species/inventory_items.xml', scripts)
            locales = game / 'localization/en'
            locales.mkdir(parents=True)
            inventory = workbook(['sysname', 'content', 'description'],
                                 [['rifle_item', 'Localized rifle', 'Description'],
                                  ['shotgun_item', 'Localized shotgun', 'Shotgun description'],
                                  ['rocket_item', 'Rocket', 'Rocket description']])
            with zipfile.ZipFile(locales / 'basis_en.pak', 'w') as archive:
                archive.writestr('locale/inventory_items.xml', inventory)
                archive.writestr('locale/guns.xml', workbook(['sysname', 'content'],
                                    [['inf_m72law_add', 'Hint'], ['stock', 'Old']]))
            with zipfile.ZipFile(locales / 'patch_002_dlc3.pak', 'w') as archive:
                archive.writestr('locale/guns.xml', workbook(['sysname', 'content'],
                                    [['inf_m72law_add', 'Updated hint'], ['stock', 'New']]))
            entries, sources = mod_entries(game)
            base = f'mods/{MOD}/localization/en/'
            self.assertEqual(entries[base + 'basis_en/locale/guns.xml'],
                             entries[base + 'dlc3/locale/guns.xml'])
            rows = table(entries[base + 'basis_en/locale/guns.xml'])[3]
            contents = {row['sysname']: row['content'] for row in rows}
            self.assertEqual(contents['stock'], 'New')
            self.assertEqual(contents['rifle_add'], 'Updated hint')
            self.assertEqual(contents['shotgun'], 'Localized shotgun')
            self.assertEqual(contents['shotgun_add'], 'Updated hint')
            self.assertNotIn('rocket', contents)
            self.assertNotIn('rifle_enemy', contents)
            self.assertEqual(sources['locales']['en']['added_rows'], 6)


if __name__ == '__main__':
    unittest.main()
