#!/usr/bin/env python3
"""Lossless ANIM v1 codec and hierarchy-aware locomotion/action compositor.

The compose command preserves action upper-body tracks and compensates thigh
transforms using the model's verified skeleton and wxyz quaternion convention.
Generated clips are prototypes that still require visual gameplay validation.
"""

from __future__ import annotations

import argparse
import hashlib
import math
import struct
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from inspect_infantry_animation import AnimFormatError, inspect_file
from inspect_model_skeleton import read_skeleton


HEADER = struct.Struct("<4sIfI")
U32 = struct.Struct("<I")
POSITION = struct.Struct("<4f")
ROTATION = struct.Struct("<5f")
MAGIC = b"ANIM"
VERSION = 1
MAX_FILE_BYTES = 256 * 1024 * 1024


@dataclass(frozen=True)
class Track:
    name: str
    name_bytes: bytes
    positions: tuple[tuple[float, float, float, float], ...]
    rotations: tuple[tuple[float, float, float, float, float], ...]


@dataclass(frozen=True)
class Clip:
    duration: float
    tracks: tuple[Track, ...]
    version: int = VERSION

    @property
    def by_name(self) -> dict[str, Track]:
        return {track.name: track for track in self.tracks}


def read_clip(path: Path) -> Clip:
    """Read a validated ANIM v1 clip while retaining each name's exact bytes."""
    inspect_file(path)
    raw = path.read_bytes()
    if len(raw) > MAX_FILE_BYTES:
        raise AnimFormatError(f"file size {len(raw)} exceeds limit {MAX_FILE_BYTES}")
    magic, version, duration, track_count = HEADER.unpack_from(raw)
    if magic != MAGIC or version != VERSION:
        raise AnimFormatError("inspector accepted a format this reader cannot represent")
    offset = HEADER.size
    tracks: list[Track] = []
    for index in range(track_count):
        nul = raw.find(b"\0", offset)
        if nul < 0:
            raise AnimFormatError(f"track {index}: missing NUL-terminated name")
        name_bytes = raw[offset:nul]
        name = name_bytes.decode("utf-8")
        offset = nul + 1

        def read_records(record: struct.Struct, label: str) -> tuple[tuple[float, ...], ...]:
            nonlocal offset
            if offset + U32.size > len(raw):
                raise AnimFormatError(f"truncated {label} count")
            count = U32.unpack_from(raw, offset)[0]
            offset += U32.size
            byte_count = count * record.size
            end = offset + byte_count
            if end > len(raw):
                raise AnimFormatError(f"truncated {label} records")
            values = tuple(record.unpack_from(raw, pos) for pos in range(offset, end, record.size))
            offset = end
            return values

        positions = read_records(POSITION, f"{name} positions")
        rotations = read_records(ROTATION, f"{name} rotations")
        tracks.append(Track(name, name_bytes, positions, rotations))
    if offset != len(raw):
        raise AnimFormatError(f"trailing data: {len(raw) - offset} bytes")
    return Clip(duration, tuple(tracks), version)


def encode_clip(clip: Clip) -> bytes:
    """Encode a clip in the observed ANIM v1 layout without altering keys."""
    if clip.version != VERSION:
        raise AnimFormatError(f"unsupported ANIM version {clip.version}")
    if not math.isfinite(clip.duration) or clip.duration < 0:
        raise AnimFormatError(f"invalid duration {clip.duration!r}")
    if len(clip.tracks) > 16_384:
        raise AnimFormatError("track count exceeds inspector limit")
    out = bytearray(HEADER.pack(MAGIC, VERSION, clip.duration, len(clip.tracks)))
    seen: set[str] = set()
    total_keys = 0
    for track in clip.tracks:
        name_bytes = track.name_bytes
        try:
            name = name_bytes.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise AnimFormatError(f"track name bytes are not valid UTF-8: {name_bytes.hex()}") from exc
        if len(name_bytes) > 4096:
            raise AnimFormatError(f"track name exceeds 4096 bytes: {name!r}")
        if name != track.name or not name_bytes or b"\0" in name_bytes:
            raise AnimFormatError(f"track name does not round-trip as UTF-8: {track.name!r}")
        if name in seen:
            raise AnimFormatError(f"duplicate track name {name!r}")
        seen.add(name)
        out.extend(name_bytes)
        out.append(0)
        total_keys += len(track.positions) + len(track.rotations)
        if total_keys > 8_000_000:
            raise AnimFormatError("total key count exceeds inspector limit")
        for records, record, label in (
            (track.positions, POSITION, f"{name} position"),
            (track.rotations, ROTATION, f"{name} rotation"),
        ):
            if len(records) > 2_000_000:
                raise AnimFormatError(f"{label} count exceeds inspector limit")
            out.extend(U32.pack(len(records)))
            for index, values in enumerate(records):
                if len(values) != len(record.unpack(bytes(record.size))):
                    raise AnimFormatError(f"{label} key {index} has wrong component count")
                if not all(math.isfinite(float(value)) for value in values):
                    raise AnimFormatError(f"{label} key {index} contains a non-finite value")
                time = float(values[0])
                if time < -1e-4 or time > clip.duration + 1e-4:
                    raise AnimFormatError(f"{label} key {index} time {time} lies outside clip duration")
                if index and time + 1e-4 < float(records[index - 1][0]):
                    raise AnimFormatError(f"{label} key times decrease at key {index}")
                out.extend(record.pack(*values))
    if len(out) > MAX_FILE_BYTES:
        raise AnimFormatError(f"encoded file exceeds {MAX_FILE_BYTES} bytes")
    return bytes(out)


def write_clip(path: Path, clip: Clip) -> str:
    """Write then validate an output clip; return its SHA-256 digest."""
    data = encode_clip(clip)
    path.write_bytes(data)
    inspect_file(path)
    return hashlib.sha256(data).hexdigest()


def lerp(a: Sequence[float], b: Sequence[float], t: float) -> tuple[float, ...]:
    if len(a) != len(b):
        raise ValueError("vectors must have equal lengths")
    t = min(1.0, max(0.0, float(t)))
    return tuple(float(x) + (float(y) - float(x)) * t for x, y in zip(a, b))


def sample_linear(keys: Sequence[Sequence[float]], time: float) -> tuple[float, ...] | None:
    """Sample time/value keys by linear interpolation; endpoints are clamped."""
    if not keys:
        return None
    if any(len(key) < 2 for key in keys):
        raise ValueError("each key needs time and at least one value")
    t = float(time)
    if not math.isfinite(t):
        raise ValueError("sample time must be finite")
    if t <= keys[0][0]:
        return tuple(float(v) for v in keys[0][1:])
    for left, right in zip(keys, keys[1:]):
        if t <= right[0]:
            span = float(right[0]) - float(left[0])
            if span <= 0:
                return tuple(float(v) for v in right[1:])
            return lerp(left[1:], right[1:], (t - float(left[0])) / span)
    return tuple(float(v) for v in keys[-1][1:])


def sample_slerp(
    keys: Sequence[Sequence[float]], time: float, *, order: str
) -> tuple[float, float, float, float] | None:
    """Sample time/quaternion keys using slerp and endpoint clamping."""
    if not keys:
        return None
    if any(len(key) != 5 for key in keys):
        raise ValueError("rotation keys must contain time and four quaternion components")
    t = float(time)
    if not math.isfinite(t):
        raise ValueError("sample time must be finite")
    if t <= keys[0][0]:
        return tuple(float(v) for v in keys[0][1:])  # type: ignore[return-value]
    for left, right in zip(keys, keys[1:]):
        if t <= right[0]:
            span = float(right[0]) - float(left[0])
            if span <= 0:
                return tuple(float(v) for v in right[1:])  # type: ignore[return-value]
            return quat_slerp(left[1:], right[1:], (t - float(left[0])) / span, order=order)
    return tuple(float(v) for v in keys[-1][1:])  # type: ignore[return-value]


def _quat_to_xyzw(q: Sequence[float], order: str) -> tuple[float, float, float, float]:
    if len(q) != 4:
        raise ValueError("quaternion must have four components")
    if order == "xyzw":
        return tuple(float(v) for v in q)  # type: ignore[return-value]
    if order == "wxyz":
        w, x, y, z = (float(v) for v in q)
        return x, y, z, w
    raise ValueError("order must be explicitly 'xyzw' or 'wxyz'")


def _xyzw_to_order(q: Sequence[float], order: str) -> tuple[float, float, float, float]:
    x, y, z, w = q
    if order == "xyzw":
        return x, y, z, w
    if order == "wxyz":
        return w, x, y, z
    raise ValueError("order must be explicitly 'xyzw' or 'wxyz'")


def quat_multiply(a: Sequence[float], b: Sequence[float], *, order: str) -> tuple[float, float, float, float]:
    """Hamilton product for active rotations; output uses the requested order."""
    ax, ay, az, aw = _quat_to_xyzw(a, order)
    bx, by, bz, bw = _quat_to_xyzw(b, order)
    return _xyzw_to_order(
        (
            aw * bx + ax * bw + ay * bz - az * by,
            aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw,
            aw * bw - ax * bx - ay * by - az * bz,
        ),
        order,
    )


def quat_rotate_vector(q: Sequence[float], vector: Sequence[float], *, order: str) -> tuple[float, float, float]:
    """Rotate a 3-vector by an active quaternion using Hamilton convention."""
    if len(vector) != 3:
        raise ValueError("vector must have three components")
    x, y, z, w = _quat_to_xyzw(q, order)
    norm = math.sqrt(x * x + y * y + z * z + w * w)
    if norm == 0.0:
        raise ValueError("zero-length quaternion")
    x, y, z, w = x / norm, y / norm, z / norm, w / norm
    vx, vy, vz = (float(v) for v in vector)
    # q * (v, 0) * conjugate(q), expanded to avoid temporary quaternions.
    tx = 2.0 * (y * vz - z * vy)
    ty = 2.0 * (z * vx - x * vz)
    tz = 2.0 * (x * vy - y * vx)
    return vx + w * tx + (y * tz - z * ty), vy + w * ty + (z * tx - x * tz), vz + w * tz + (x * ty - y * tx)


@dataclass(frozen=True)
class RigidTransform:
    position: tuple[float, float, float]
    rotation: tuple[float, float, float, float]


def compose_transform(parent: RigidTransform, local: RigidTransform, *, order: str) -> RigidTransform:
    """Compose parent/world with local, in the game's verified rigid convention."""
    rotated = quat_rotate_vector(parent.rotation, local.position, order=order)
    return RigidTransform(
        tuple(a + b for a, b in zip(parent.position, rotated)),
        quat_multiply(parent.rotation, local.rotation, order=order),
    )


def inverse_transform(transform: RigidTransform, *, order: str) -> RigidTransform:
    """Invert a rigid transform (the ANIM composition path does not apply scale)."""
    x, y, z, w = _quat_to_xyzw(transform.rotation, order)
    norm_sq = x * x + y * y + z * z + w * w
    if norm_sq == 0.0:
        raise ValueError("zero-length quaternion")
    inverse_q_xyzw = (-x / norm_sq, -y / norm_sq, -z / norm_sq, w / norm_sq)
    inverse_q = _xyzw_to_order(inverse_q_xyzw, order)
    inverse_position = quat_rotate_vector(inverse_q, tuple(-v for v in transform.position), order=order)
    return RigidTransform(inverse_position, inverse_q)


def relative_transform(parent: RigidTransform, world: RigidTransform, *, order: str) -> RigidTransform:
    """Return the local transform L satisfying parent * L == world."""
    return compose_transform(inverse_transform(parent, order=order), world, order=order)


def _sample_transform(track: Track, time: float, *, order: str) -> RigidTransform:
    position = sample_linear(track.positions, time)
    rotation = sample_slerp(track.rotations, time, order=order)
    if position is None or rotation is None or len(position) != 3:
        raise AnimFormatError(f"track {track.name!r} lacks a usable position or rotation channel")
    return RigidTransform(tuple(position), rotation)


def _local_at(clip: Clip, track_name: str, time: float, *, order: str) -> RigidTransform:
    try:
        track = clip.by_name[track_name]
    except KeyError as exc:
        raise AnimFormatError(f"required animation track {track_name!r} is missing") from exc
    return _sample_transform(track, time, order=order)


def _world_at(
    clip: Clip,
    node_name: str,
    parents: dict[str, str | None],
    time: float,
    *,
    order: str,
) -> RigidTransform:
    chain: list[str] = []
    current: str | None = node_name
    while current is not None:
        chain.append(current)
        current = parents.get(current)
    result = RigidTransform((0.0, 0.0, 0.0), (1.0, 0.0, 0.0, 0.0) if order == "wxyz" else (0.0, 0.0, 0.0, 1.0))
    for name in reversed(chain):
        # The humanoid animation tracks are parent-local; the separate model
        # root (USA_soldier_1) is not an animated joint and is not in this chain.
        result = compose_transform(result, _local_at(clip, name, time, order=order), order=order)
    return result


def _channel_sample(keys: Sequence[Sequence[float]], time: float, *, rotation: bool, order: str) -> tuple[float, ...]:
    if rotation:
        value = sample_slerp(keys, time, order=order)
    else:
        value = sample_linear(keys, time)
    if value is None:
        raise AnimFormatError("cannot sample empty animation channel")
    return value


def _looped_records(
    keys: Sequence[Sequence[float]],
    loop_duration: float,
    output_duration: float,
    *,
    rotation: bool,
    order: str,
) -> tuple[tuple[float, ...], ...]:
    """Repeat source key cadence, retaining a single key at each cycle seam."""
    if not keys or loop_duration <= 0:
        raise AnimFormatError("loop source needs keys and a positive duration")
    result: list[tuple[float, ...]] = []
    cycle_count = int(math.floor(output_duration / loop_duration)) + 1
    for cycle in range(cycle_count):
        shift = cycle * loop_duration
        for key_index, key in enumerate(keys):
            time = shift + float(key[0])
            if time > output_duration + 1e-6:
                continue
            result.append((time, *(float(value) for value in key[1:])))
    if not result:
        raise AnimFormatError("loop produced no animation keys")
    if result[-1][0] < output_duration - 1e-6:
        quotient = output_duration / loop_duration
        phase = output_duration - math.floor(quotient) * loop_duration
        if phase <= 1e-6 and output_duration > 0:
            phase = loop_duration
        value = _channel_sample(keys, phase, rotation=rotation, order=order)
        result.append((output_duration, *value))
    # float32 source times can land on the seam with tiny representational
    # differences. Collapse exact-near duplicates, keeping the later authored
    # sample, so the emitted timeline remains monotonic and unambiguous.
    collapsed: list[tuple[float, ...]] = []
    for key in result:
        if collapsed and abs(key[0] - collapsed[-1][0]) <= 1e-6:
            collapsed[-1] = key
        else:
            collapsed.append(key)
    return tuple(collapsed)


def _loop_phase(time: float, loop_duration: float) -> float:
    phase = time - math.floor(time / loop_duration) * loop_duration
    if phase <= 1e-6:
        return 0.0
    return phase


def _is_constant_channel(keys: Sequence[Sequence[float]], tolerance: float = 1e-6) -> bool:
    if len(keys) <= 1:
        return True
    first = keys[0][1:]
    return all(all(abs(float(a) - float(b)) <= tolerance for a, b in zip(first, key[1:])) for key in keys[1:])


def compose_locomotion_legs(
    action: Clip,
    locomotion: Clip,
    parents: dict[str, str | None],
    *,
    spine: str = "Bip01_Spine",
    thighs: tuple[str, str] = ("Bip01_L_Thigh", "Bip01_R_Thigh"),
    order: str = "wxyz",
    preserve_static_descendants: bool = False,
) -> Clip:
    """Keep the action upper body/root and layer a looping run below Spine.

    Each thigh's local transform is solved so its world transform matches the
    locomotion clip while its parent Spine remains from the action. Animated
    descendants use their native locomotion channels. Static action-only
    descendants remain untouched.
    """
    if order not in ("wxyz", "xyzw"):
        raise ValueError("order must be 'wxyz' or 'xyzw'")
    if locomotion.duration <= 0:
        raise AnimFormatError("locomotion duration must be positive")
    action_tracks = action.by_name
    locomotion_tracks = locomotion.by_name
    if spine not in parents:
        raise AnimFormatError(f"model has no spine node {spine!r}")

    lower_nodes: set[str] = set()
    for thigh in thighs:
        if thigh not in parents:
            raise AnimFormatError(f"model has no thigh node {thigh!r}")
        if parents[thigh] != spine:
            raise AnimFormatError(f"expected {thigh!r} to be a direct child of {spine!r}")
        lower_nodes.add(thigh)
        changed = True
        while changed:
            changed = False
            for name, parent in parents.items():
                if parent in lower_nodes and name not in lower_nodes:
                    lower_nodes.add(name)
                    changed = True

    replacements: dict[str, Track] = {}
    for thigh in thighs:
        if thigh not in action_tracks or thigh not in locomotion_tracks:
            raise AnimFormatError(f"both clips must contain thigh track {thigh!r}")
        run_thigh = locomotion_tracks[thigh]
        position_times = _looped_records(
            run_thigh.positions, locomotion.duration, action.duration,
            rotation=False, order=order,
        )
        rotation_times = _looped_records(
            run_thigh.rotations, locomotion.duration, action.duration,
            rotation=True, order=order,
        )

        def compensated(at: float) -> RigidTransform:
            action_spine_world = _world_at(action, spine, parents, at, order=order)
            run_spine_world = _world_at(
                locomotion, spine, parents, _loop_phase(at, locomotion.duration), order=order
            )
            run_thigh_local = _local_at(
                locomotion, thigh, _loop_phase(at, locomotion.duration), order=order
            )
            run_thigh_world = compose_transform(run_spine_world, run_thigh_local, order=order)
            return relative_transform(action_spine_world, run_thigh_world, order=order)

        pos_records = tuple((key[0], *compensated(key[0]).position) for key in position_times)
        rot_records = tuple((key[0], *compensated(key[0]).rotation) for key in rotation_times)
        original = action_tracks[thigh]
        replacements[thigh] = Track(original.name, original.name_bytes, pos_records, rot_records)

    for node_name in lower_nodes - set(thighs):
        original = action_tracks.get(node_name)
        source = locomotion_tracks.get(node_name)
        if original is None:
            continue
        if source is None:
            if not (_is_constant_channel(original.positions) and _is_constant_channel(original.rotations)):
                raise AnimFormatError(f"animated lower-body track {node_name!r} is missing from locomotion")
            continue
        # Preserve action-only/static descendants byte-for-byte when the gait
        # track is static too (notably the toe nubs in the backpedal clip).
        if (preserve_static_descendants
                and _is_constant_channel(original.positions) and _is_constant_channel(original.rotations)
                and _is_constant_channel(source.positions) and _is_constant_channel(source.rotations)):
            continue
        replacements[node_name] = Track(
            original.name,
            original.name_bytes,
            _looped_records(source.positions, locomotion.duration, action.duration,
                            rotation=False, order=order),
            _looped_records(source.rotations, locomotion.duration, action.duration,
                            rotation=True, order=order),
        )

    return Clip(
        action.duration,
        tuple(replacements.get(track.name, track) for track in action.tracks),
        action.version,
    )


def retime_clip(clip: Clip, duration: float) -> Clip:
    """Uniformly retime all channels to a new duration, retaining phase zero.

    This is used for the documented backpedal cycle: native key values and
    channel sampling are unchanged, while each authored timestamp is scaled by
    ``duration / clip.duration``. The returned clip remains a single loop.
    """
    if not math.isfinite(duration) or duration <= 0 or clip.duration <= 0:
        raise AnimFormatError("retime durations must be positive and finite")
    scale = duration / clip.duration
    tracks = tuple(
        Track(
            track.name,
            track.name_bytes,
            tuple((float(key[0]) * scale, *key[1:]) for key in track.positions),
            tuple((float(key[0]) * scale, *key[1:]) for key in track.rotations),
        )
        for track in clip.tracks
    )
    return Clip(duration, tracks, clip.version)


def _parent_map(model_path: Path) -> dict[str, str | None]:
    skeleton = read_skeleton(model_path)
    return {node["name"]: node["parent"] for node in skeleton["nodes"]}


def quat_slerp(a: Sequence[float], b: Sequence[float], t: float, *, order: str) -> tuple[float, float, float, float]:
    """Shortest-arc normalized quaternion slerp; component order is mandatory."""
    qa = _quat_to_xyzw(a, order)
    qb = _quat_to_xyzw(b, order)
    t = min(1.0, max(0.0, float(t)))
    na = math.sqrt(sum(v * v for v in qa))
    nb = math.sqrt(sum(v * v for v in qb))
    if na == 0.0 or nb == 0.0:
        raise ValueError("zero-length quaternion")
    qa = tuple(v / na for v in qa)
    qb = tuple(v / nb for v in qb)
    dot = sum(x * y for x, y in zip(qa, qb))
    if dot < 0.0:
        qb = tuple(-v for v in qb)
        dot = -dot
    dot = min(1.0, max(-1.0, dot))
    if dot > 0.9995:
        result = tuple(x + t * (y - x) for x, y in zip(qa, qb))
        norm = math.sqrt(sum(v * v for v in result))
        result = tuple(v / norm for v in result)
    else:
        theta = math.acos(dot)
        sin_theta = math.sin(theta)
        wa = math.sin((1.0 - t) * theta) / sin_theta
        wb = math.sin(t * theta) / sin_theta
        result = tuple(wa * x + wb * y for x, y in zip(qa, qb))
    return _xyzw_to_order(result, order)


def _roundtrip(args: argparse.Namespace) -> int:
    source = Path(args.source)
    destination = Path(args.destination)
    clip = read_clip(source)
    digest = write_clip(destination, clip)
    original_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    exact = original_hash == digest
    print(f"source_sha256={original_hash}")
    print(f"output_sha256={digest}")
    print(f"byte_exact={str(exact).lower()} duration={clip.duration:.9g} tracks={len(clip.tracks)}")
    return 0 if exact else 1


def _compose(args: argparse.Namespace) -> int:
    action_path = Path(args.action)
    locomotion_path = Path(args.locomotion)
    model_path = Path(args.model)
    output_path = Path(args.output)
    action = read_clip(action_path)
    locomotion = read_clip(locomotion_path)
    parents = _parent_map(model_path)
    combined = compose_locomotion_legs(
        action,
        locomotion,
        parents,
        spine=args.spine,
        order=args.quaternion_order,
    )
    digest = write_clip(output_path, combined)
    print(f"output={output_path}")
    print(f"duration={combined.duration:.9g} tracks={len(combined.tracks)} sha256={digest}")
    print(f"action_sha256={hashlib.sha256(action_path.read_bytes()).hexdigest()}")
    print(f"locomotion_sha256={hashlib.sha256(locomotion_path.read_bytes()).hexdigest()}")
    print(f"model_sha256={hashlib.sha256(model_path.read_bytes()).hexdigest()}")
    print(f"hierarchy_spine={args.spine} quaternion_order={args.quaternion_order}")
    print("scope=offline prototype; verify in-game before use")
    return 0


def _retime(args: argparse.Namespace) -> int:
    source = Path(args.source)
    destination = Path(args.destination)
    clip = retime_clip(read_clip(source), args.duration)
    digest = write_clip(destination, clip)
    print(f"output={destination}")
    print(f"duration={clip.duration:.9g} sha256={digest}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    roundtrip = sub.add_parser("roundtrip", help="validate and byte-exactly rewrite one clip")
    roundtrip.add_argument("source")
    roundtrip.add_argument("destination")
    compose = sub.add_parser("compose", help="layer looping run legs under a stock action clip")
    compose.add_argument("--action", required=True, help="stock ANIM action clip")
    compose.add_argument("--locomotion", required=True, help="looping ANIM locomotion clip")
    compose.add_argument("--model", required=True, help="MODL v7 skeleton used to validate node parents")
    compose.add_argument("--output", required=True, help="new output path; source assets remain untouched")
    compose.add_argument("--spine", default="Bip01_Spine")
    compose.add_argument("--quaternion-order", choices=("wxyz", "xyzw"), default="wxyz")
    retime = sub.add_parser("retime", help="uniformly retime one clip while preserving key values")
    retime.add_argument("--source", required=True)
    retime.add_argument("--duration", required=True, type=float)
    retime.add_argument("--destination", required=True)
    args = parser.parse_args(argv)
    if args.command == "roundtrip":
        return _roundtrip(args)
    if args.command == "compose":
        return _compose(args)
    if args.command == "retime":
        return _retime(args)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
