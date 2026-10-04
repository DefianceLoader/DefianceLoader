#!/usr/bin/env python3
"""Strict read-only inspector for infantry ANIM v1 files.

The v1 layout is intentionally decoded without normalization or rewriting:
header <4sIfI, then named position/rotation tracks, then exact EOF.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import BinaryIO


HEADER = struct.Struct("<4sIfI")
U32 = struct.Struct("<I")
POSITION = struct.Struct("<4f")
ROTATION = struct.Struct("<5f")
MAGIC = b"ANIM"
VERSION = 1
TIME_EPSILON = 1e-4
MAX_TRACKS = 16_384
MAX_KEYS_PER_TRACK = 2_000_000
MAX_TOTAL_KEYS = 8_000_000
MAX_FILE_BYTES = 256 * 1024 * 1024
MAX_NAME_BYTES = 4096


class AnimFormatError(ValueError):
    pass


@dataclass
class TrackInfo:
    name: str
    name_bytes_hex: str | None
    position_keys: int
    rotation_keys: int
    position_tail: tuple[float, float, float, float] | None
    rotation_tail: tuple[float, float, float, float, float] | None


@dataclass
class AnimationInfo:
    path: str
    magic: str
    version: int
    duration: float
    track_count: int
    position_key_count: int
    rotation_key_count: int
    non_ascii_name_count: int
    tracks: list[TrackInfo]
    sha256: str


def _read_exact(stream: BinaryIO, size: int, what: str) -> bytes:
    if size < 0:
        raise AnimFormatError(f"invalid negative size for {what}")
    data = stream.read(size)
    if len(data) != size:
        raise AnimFormatError(f"truncated {what}: needed {size} bytes, got {len(data)}")
    return data


def _read_track_name(stream: BinaryIO, index: int) -> tuple[str, bytes]:
    name = bytearray()
    for _ in range(MAX_NAME_BYTES + 1):
        b = stream.read(1)
        if not b:
            raise AnimFormatError(f"truncated track {index} name (missing NUL terminator)")
        if b == b"\0":
            if not name:
                raise AnimFormatError(f"track {index} has an empty name")
            try:
                return name.decode("utf-8"), bytes(name)
            except UnicodeDecodeError as exc:
                raise AnimFormatError(f"track {index} name is not valid UTF-8") from exc
        name.extend(b)
    raise AnimFormatError(f"track {index} name exceeds {MAX_NAME_BYTES} bytes")


def _read_keys(
    stream: BinaryIO,
    count: int,
    record: struct.Struct,
    *,
    duration: float,
    label: str,
) -> tuple[tuple[float, ...] | None, int]:
    if count > MAX_KEYS_PER_TRACK:
        raise AnimFormatError(f"{label} count {count} exceeds limit {MAX_KEYS_PER_TRACK}")
    # Check available bytes before the loop; avoids pathological work on corrupt counts.
    here = stream.tell()
    stream.seek(0, 2)
    remaining = stream.tell() - here
    stream.seek(here)
    needed = count * record.size
    if needed > remaining:
        raise AnimFormatError(f"{label} records need {needed} bytes, only {remaining} remain")

    tail: tuple[float, ...] | None = None
    previous_time = -math.inf
    for key_index in range(count):
        values = record.unpack(_read_exact(stream, record.size, f"{label} key {key_index}"))
        if not all(math.isfinite(value) for value in values):
            raise AnimFormatError(f"{label} key {key_index} contains a non-finite value")
        time = values[0]
        if time + TIME_EPSILON < previous_time:
            raise AnimFormatError(
                f"{label} key times decrease at key {key_index}: {time} < {previous_time}"
            )
        if time < -TIME_EPSILON or time > duration + TIME_EPSILON:
            raise AnimFormatError(
                f"{label} key {key_index} time {time} lies outside [0, {duration}]"
            )
        previous_time = time
        tail = values
    return tail, count


def inspect_file(path: Path) -> AnimationInfo:
    size = path.stat().st_size
    if size > MAX_FILE_BYTES:
        raise AnimFormatError(f"file size {size} exceeds limit {MAX_FILE_BYTES}")
    raw = path.read_bytes()
    if len(raw) < HEADER.size:
        raise AnimFormatError(f"header truncated: expected {HEADER.size} bytes, got {len(raw)}")
    magic, version, duration, track_count = HEADER.unpack_from(raw)
    if magic != MAGIC:
        raise AnimFormatError(f"unsupported magic {magic!r}; expected {MAGIC!r}")
    if version != VERSION:
        raise AnimFormatError(f"unsupported ANIM version {version}; inspector supports v{VERSION}")
    # Zero-duration pose clips exist in the shipped set; their keys must still
    # satisfy the same finite/time-range checks below.
    if not math.isfinite(duration) or duration < 0.0:
        raise AnimFormatError(f"invalid duration {duration!r}")
    if track_count > MAX_TRACKS:
        raise AnimFormatError(f"track count {track_count} exceeds limit {MAX_TRACKS}")

    import io

    stream = io.BytesIO(raw)
    stream.seek(HEADER.size)
    names: set[str] = set()
    tracks: list[TrackInfo] = []
    non_ascii_names = 0
    total_position = 0
    total_rotation = 0
    for index in range(track_count):
        name, name_bytes = _read_track_name(stream, index)
        if name in names:
            raise AnimFormatError(f"duplicate track name {name!r}")
        names.add(name)
        non_ascii = any(byte >= 0x80 for byte in name_bytes)
        non_ascii_names += int(non_ascii)
        position_count = U32.unpack(_read_exact(stream, U32.size, f"{name} position count"))[0]
        position_tail, _ = _read_keys(
            stream,
            position_count,
            POSITION,
            duration=duration,
            label=f"{name} position",
        )
        rotation_count = U32.unpack(_read_exact(stream, U32.size, f"{name} rotation count"))[0]
        rotation_tail, _ = _read_keys(
            stream,
            rotation_count,
            ROTATION,
            duration=duration,
            label=f"{name} rotation",
        )
        total_position += position_count
        total_rotation += rotation_count
        if total_position + total_rotation > MAX_TOTAL_KEYS:
            raise AnimFormatError(f"total key count exceeds limit {MAX_TOTAL_KEYS}")
        tracks.append(
            TrackInfo(
                name,
                name_bytes.hex() if non_ascii else None,
                position_count,
                rotation_count,
                position_tail,
                rotation_tail,
            )
        )
    if stream.tell() != len(raw):
        raise AnimFormatError(f"trailing data: {len(raw) - stream.tell()} bytes after final track")

    return AnimationInfo(
        str(path),
        magic.decode("ascii"),
        version,
        duration,
        track_count,
        total_position,
        total_rotation,
        non_ascii_names,
        tracks,
        hashlib.sha256(raw).hexdigest(),
    )


def expand_inputs(inputs: list[Path]) -> list[Path]:
    paths: set[Path] = set()
    for item in inputs:
        if item.is_dir():
            paths.update(p for p in item.rglob("*.anim") if p.is_file())
        elif item.is_file():
            paths.add(item)
        else:
            raise FileNotFoundError(item)
    return sorted(paths, key=lambda p: str(p).casefold())


def print_text(info: AnimationInfo) -> None:
    print(
        f"{info.path}: {info.magic} v{info.version} duration={info.duration:.6g} "
        f"tracks={info.track_count} position_keys={info.position_key_count} "
        f"rotation_keys={info.rotation_key_count} non_ascii_names={info.non_ascii_name_count} "
        f"sha256={info.sha256}"
    )
    for track in info.tracks:
        print(
            f"  {track.name}"
            f"{f' [utf8-bytes={track.name_bytes_hex}]' if track.name_bytes_hex else ''}: "
            f"position={track.position_keys} tail={track.position_tail!r}; "
            f"rotation={track.rotation_keys} tail={track.rotation_tail!r}"
        )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="+", type=Path, help="ANIM file(s) or directories")
    parser.add_argument("--json", action="store_true", help="emit readable JSON for valid files")
    args = parser.parse_args(argv)
    try:
        paths = expand_inputs(args.paths)
    except OSError as exc:
        print(f"input error: {exc}", file=sys.stderr)
        return 2
    failures: list[tuple[Path, str]] = []
    infos: list[AnimationInfo] = []
    for path in paths:
        try:
            infos.append(inspect_file(path))
        except (OSError, AnimFormatError) as exc:
            failures.append((path, str(exc)))
    if args.json:
        print(json.dumps({"valid": [asdict(info) for info in infos],
                          "failures": [{"path": str(path), "error": error}
                                       for path, error in failures]}, indent=2))
    else:
        for info in infos:
            print_text(info)
        for path, error in failures:
            print(f"INVALID {path}: {error}", file=sys.stderr)
        print(f"summary: inspected={len(paths)} valid_v1={len(infos)} failures={len(failures)}")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
