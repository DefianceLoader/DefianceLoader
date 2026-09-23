"""Game-independent tests for the companion layout/package transformation."""
import struct
import tempfile
import unittest
from pathlib import Path
import zipfile
from package_squad_scroll import (layout, vehicle_layout, resources, dds,
                                  RESOURCE, VEHICLE_RESOURCE)


def fixture(vehicle=False):
    rows = ['name\ttype\tregion\tlink\ttip\tproperty\tvalue']
    for i in range(6):
        x, y = 1 + i % 3 * 152, 465 + i // 3 * 76
        rows.append(f'weapon_slot_{i+1}\twidget\t{x},{y},{x+148},{y+76}\t\t\t\t')
        x = 1 + i * 76
        rows.append(f'ammo_slot_{i+1}\twidget\t{x},617,{x+72},689\t\t\t\t')
    for i in range(5):
        rows.append(f'upgrade_slot_{i+1}\twidget\t377,{49+i*74},449,{121+i*74}\t\t\t\t')
        if not vehicle:
            rows.append(f'training_slot_{i+1}\twidget\t{42+i*74},103,{118+i*74},191\t\t\t\t')
    return ('\r\n'.join(rows) + '\r\n').encode()


class PackageTests(unittest.TestCase):
    def test_infantry_panel_adds_four_sliders(self):
        before = fixture()
        after = layout(before)
        self.assertTrue(after.startswith(before))
        self.assertEqual(after.count(b'\tslider\t'), 4)
        self.assertIn(b'1, 461, 453, 465', after)
        self.assertIn(b'1, 689, 453, 693', after)
        self.assertIn(b'1, 192, 453, 196', after)
        # The upgrade slider is vertical, in the gutter beside the column.
        self.assertIn(b'df_upgrades\tslider\t 449, 49, 453, 417', after)
        self.assertEqual(after.count(b'\tdirection\tvertical'), 1)
        # enabled like the game's own sliders, or clicks and drags are ignored
        added = after[len(before):]
        self.assertEqual(added.count(b'\tenabled\ttrue'), 4)
        self.assertNotIn(b'\tenabled\tfalse', added)
        self.assertIn(b'df_perks', after)
        self.assertIn(b'df_upgrades', after)

    def test_vehicle_panel_adds_three_sliders_and_no_perk_row(self):
        before = fixture(vehicle=True)
        after = vehicle_layout(before)
        self.assertTrue(after.startswith(before))
        self.assertEqual(after.count(b'\tslider\t'), 3)
        # the same gutter as the infantry panel; nothing else is vertical
        self.assertIn(b'df_upgrades\tslider\t 449, 49, 453, 417', after)
        self.assertEqual(after.count(b'\tdirection\tvertical'), 1)
        self.assertNotIn(b'df_perks', after)
        self.assertEqual(after[len(before):].count(b'\tenabled\ttrue'), 3)

    def test_rejects_changed_geometry_missing_slots_and_second_application(self):
        for data in [fixture().replace(b'465', b'464'), fixture().replace(b'weapon_slot_6', b'other'), layout(fixture())]:
            with self.assertRaises(ValueError): layout(data)
        for data in [fixture(vehicle=True).replace(b'465', b'464'), vehicle_layout(fixture(vehicle=True))]:
            with self.assertRaises(ValueError): vehicle_layout(data)

    def test_newest_shared_panel_is_used_in_every_overlay(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name, marker in [('basis.pak', b'base'), ('patch_002.pak', b'patched'),
                                 ('patch_010_dlc.pak', b'dlc'), ('patch_018.pak', b'latest')]:
                with zipfile.ZipFile(root / name, 'w') as z:
                    z.writestr(RESOURCE, fixture() + marker + b'\twidget\t0,0,1,1\r\n')
            layers, sources = resources(root, RESOURCE, layout)
            self.assertIn(b'latest\twidget', layers['basis'])
            self.assertEqual(layers['basis'], layers['dlc'])
            self.assertEqual(sources['basis']['archive'], 'patch_018.pak')
            self.assertEqual(sources['dlc']['archive'], 'patch_018.pak')

    def test_obsolete_basis_is_superseded_by_dlc_named_patch(self):
        old = b'\r\n'.join(row for row in fixture().split(b'\r\n') if not row.startswith(b'upgrade_slot_'))
        with self.assertRaisesRegex(ValueError, 'upgrade_slot_1'):
            layout(old)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name, data in [('basis.pak', old), ('patch_010_dlc.pak', fixture())]:
                with zipfile.ZipFile(root / name, 'w') as z: z.writestr(RESOURCE, data)
            layers, sources = resources(root, RESOURCE, layout)
            for layer in ['basis', 'dlc']:
                self.assertEqual(layers[layer], layout(fixture()))
                self.assertEqual(sources[layer]['archive'], 'patch_010_dlc.pak')

    def test_every_upgrade_widget_is_mandatory(self):
        for factory in (layout, vehicle_layout):
            for i in range(1, 6):
                target = fixture(vehicle=factory is vehicle_layout)
                with self.assertRaisesRegex(ValueError, f'upgrade_slot_{i}'):
                    factory(target.replace(f'upgrade_slot_{i}'.encode(), b'missing'))

    def test_vehicle_panel_resource(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with zipfile.ZipFile(root / 'basis.pak', 'w') as z:
                z.writestr(RESOURCE, fixture())
            with self.assertRaisesRegex(ValueError, 'VehicleInfoPanel'):
                resources(root, VEHICLE_RESOURCE, vehicle_layout)

    def test_dds_header_and_rgba_channels(self):
        data = dds(32, 4, (12, 34, 56, 255))
        self.assertEqual(data[:4], b'DDS ')
        self.assertEqual(struct.unpack_from('<II', data, 12), (4, 32))
        self.assertEqual(len(data), 128 + 32 * 4 * 4)
        self.assertEqual(data[128:132], bytes([56, 34, 12, 255]))


if __name__ == '__main__': unittest.main()
