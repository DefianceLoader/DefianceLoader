from __future__ import annotations

import hashlib
import math
import tempfile
import unittest
from pathlib import Path

from compose_infantry_animation import (
    Clip,
    RigidTransform,
    Track,
    _world_at,
    compose_locomotion_legs,
    compose_transform,
    encode_clip,
    inverse_transform,
    quat_rotate_vector,
    quat_slerp,
    read_clip,
    relative_transform,
    sample_linear,
    sample_slerp,
    write_clip,
)


class AnimationComposerTests(unittest.TestCase):
    def test_codec_is_byte_exact_for_non_ascii_track_name(self) -> None:
        clip = Clip(
            1.0,
            (
                Track(
                    "Bip01_L_Сlavicle",
                    bytes.fromhex("42697030315f4c5fd0a16c617669636c65"),
                    ((0.0, 1.0, 2.0, 3.0), (1.0, 4.0, 5.0, 6.0)),
                    ((0.0, 1.0, 0.0, 0.0, 0.0), (1.0, 0.0, 0.0, 0.0, 1.0)),
                ),
            ),
        )
        raw = encode_clip(clip)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "clip.anim"
            path.write_bytes(raw)
            parsed = read_clip(path)
            self.assertEqual(encode_clip(parsed), raw)
            self.assertEqual(parsed.tracks[0].name_bytes, clip.tracks[0].name_bytes)
            self.assertEqual(hashlib.sha256(encode_clip(parsed)).digest(), hashlib.sha256(raw).digest())

    def test_linear_sample_interpolates_and_clamps(self) -> None:
        keys = ((0.0, 0.0, 2.0), (2.0, 4.0, 6.0))
        self.assertEqual(sample_linear(keys, -1.0), (0.0, 2.0))
        self.assertEqual(sample_linear(keys, 1.0), (2.0, 4.0))
        self.assertEqual(sample_linear(keys, 3.0), (4.0, 6.0))
        self.assertIsNone(sample_linear((), 0.0))

    def test_slerp_uses_explicit_wxyz_order_and_shortest_arc(self) -> None:
        q0 = (1.0, 0.0, 0.0, 0.0)
        q90z = (math.sqrt(0.5), 0.0, 0.0, math.sqrt(0.5))
        halfway = quat_slerp(q0, q90z, 0.5, order="wxyz")
        rotated = quat_rotate_vector(halfway, (1.0, 0.0, 0.0), order="wxyz")
        expected = (math.sqrt(0.5), math.sqrt(0.5), 0.0)
        for actual, target in zip(rotated, expected):
            self.assertAlmostEqual(actual, target, places=6)
        self.assertEqual(quat_slerp(q90z, tuple(-v for v in q90z), 0.37, order="wxyz"), q90z)

    def test_slerp_sampler_handles_key_times(self) -> None:
        q0 = (1.0, 0.0, 0.0, 0.0)
        q180z = (0.0, 0.0, 0.0, 1.0)
        keys = ((0.0, *q0), (2.0, *q180z))
        middle = sample_slerp(keys, 1.0, order="wxyz")
        self.assertIsNotNone(middle)
        self.assertAlmostEqual(abs(middle[0]), math.sqrt(0.5), places=6)
        self.assertAlmostEqual(abs(middle[3]), math.sqrt(0.5), places=6)

    def test_rigid_transform_relative_and_inverse(self) -> None:
        q90z = (math.sqrt(0.5), 0.0, 0.0, math.sqrt(0.5))
        parent = RigidTransform((5.0, -2.0, 1.0), q90z)
        local = RigidTransform((2.0, 0.0, 0.0), (1.0, 0.0, 0.0, 0.0))
        world = compose_transform(parent, local, order="wxyz")
        self.assertAlmostEqual(world.position[0], 5.0)
        self.assertAlmostEqual(world.position[1], 0.0)
        recovered = relative_transform(parent, world, order="wxyz")
        for actual, expected in zip(recovered.position, local.position):
            self.assertAlmostEqual(actual, expected, places=6)
        roundtrip = compose_transform(inverse_transform(parent, order="wxyz"), world, order="wxyz")
        for actual, expected in zip(roundtrip.position, local.position):
            self.assertAlmostEqual(actual, expected, places=6)

    def test_leg_layer_matches_locomotion_world_and_preserves_action_spine(self) -> None:
        parents = {
            "Bip01": None,
            "Bip01_Pelvis": "Bip01",
            "Bip01_Spine": "Bip01_Pelvis",
            "Bip01_L_Thigh": "Bip01_Spine",
            "Bip01_L_Calf": "Bip01_L_Thigh",
        }
        identity = (1.0, 0.0, 0.0, 0.0)
        q90z = (math.sqrt(0.5), 0.0, 0.0, math.sqrt(0.5))

        def track(name, positions, rotations):
            return Track(name, name.encode("ascii"), tuple(positions), tuple(rotations))

        def make_clip(duration, spine_rotation, thigh_x, *, run_cycle=False):
            thigh_positions = (
                ((0.0, thigh_x, 0.0, 0.0), (duration / 2.0, thigh_x + 1.0, 0.0, 0.0),
                 (duration, thigh_x, 0.0, 0.0))
                if run_cycle else
                ((0.0, thigh_x, 0.0, 0.0), (duration, thigh_x + 1.0, 0.0, 0.0))
            )
            return Clip(duration, (
                track("Bip01", ((0.0, 0.0, 0.0, 0.0), (duration, 0.0, 0.0, 0.0)),
                      ((0.0, *identity), (duration, *identity))),
                track("Bip01_Pelvis", ((0.0, 0.0, 0.0, 0.0), (duration, 0.0, 0.0, 0.0)),
                      ((0.0, *identity), (duration, *identity))),
                track("Bip01_Spine", ((0.0, 0.0, 0.0, 0.0), (duration, 0.0, 0.0, 0.0)),
                      ((0.0, *spine_rotation), (duration, *spine_rotation))),
                track("Bip01_L_Thigh", thigh_positions,
                      ((0.0, *identity), (duration, *identity))),
                track("Bip01_L_Calf", ((0.0, 1.0, 0.0, 0.0), (duration, 1.0, 0.0, 0.0)),
                      ((0.0, *identity), (duration, *identity))),
            ))

        action = make_clip(2.0, q90z, 9.0)
        run = make_clip(1.0, identity, 2.0, run_cycle=True)
        combined = compose_locomotion_legs(action, run, parents, thighs=("Bip01_L_Thigh",))
        self.assertEqual(combined.duration, action.duration)
        self.assertIs(combined.by_name["Bip01_Spine"], action.by_name["Bip01_Spine"])
        self.assertIs(combined.by_name["Bip01"], action.by_name["Bip01"])
        for time in (0.0, 0.25, 0.75, 1.0, 1.5, 2.0):
            for node in ("Bip01_L_Thigh", "Bip01_L_Calf"):
                actual = _world_at(combined, node, parents, time, order="wxyz")
                phase = time % run.duration
                expected = _world_at(run, node, parents, phase, order="wxyz")
                for a, b in zip(actual.position, expected.position):
                    self.assertAlmostEqual(a, b, places=5, msg=f"{node} at t={time}: {actual.position} != {expected.position}")
                dot = abs(sum(a * b for a, b in zip(actual.rotation, expected.rotation)))
                self.assertAlmostEqual(dot, 1.0, places=5)


if __name__ == "__main__":
    unittest.main()
