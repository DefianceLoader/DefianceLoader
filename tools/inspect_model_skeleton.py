#!/usr/bin/env python3
"""Read the NODS skeleton of MODL v7; leave all mesh/material chunks untouched."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
from pathlib import Path


def read_skeleton(path: Path) -> dict:
    data = path.read_bytes()
    if len(data) < 16:
        raise ValueError("truncated MODL header")
    magic, version, chunk, count = struct.unpack_from("<4sI4sI", data)
    if (magic, version, chunk) != (b"MODL", 7, b"NODS"):
        raise ValueError("supports only MODL v7 with leading NODS chunk")
    if count > 16384:
        raise ValueError("excessive skeleton node count")
    offset = 16
    nodes = []
    names = set()

    def string() -> str:
        nonlocal offset
        end = data.find(b"\0", offset, min(len(data), offset + 4097))
        if end < 0:
            raise ValueError("missing/beyond-limit string terminator")
        result = data[offset:end].decode("utf-8")
        offset = end + 1
        return result

    for index in range(count):
        name = string()
        if not name or name in names:
            raise ValueError(f"empty/duplicate node name: {name!r}")
        names.add(name)
        if offset + 69 > len(data):
            raise ValueError(f"truncated node {name}")
        parent, *matrix, flag = struct.unpack_from("<i16fB", data, offset)
        offset += 69
        tag = string()
        if parent < -1 or parent >= index:
            raise ValueError(f"node {name} has unsupported parent index {parent}")
        if not all(math.isfinite(v) for v in matrix):
            raise ValueError(f"node {name} has non-finite bind matrix")
        nodes.append({"index": index, "name": name, "parent_index": parent,
                      "parent": nodes[parent]["name"] if parent >= 0 else None,
                      "bind_matrix": matrix, "flag": flag, "tag": tag})
    return {"path": str(path.resolve()), "sha256": hashlib.sha256(data).hexdigest(),
            "version": version, "node_count": count, "nodes": nodes,
            "next_chunk_offset": offset, "next_chunk": data[offset:offset + 4].decode("ascii"),
            "remaining_bytes": len(data) - offset}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("model", type=Path)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    result = read_skeleton(args.model)
    if args.json:
        print(json.dumps(result, indent=2, ensure_ascii=True))
    else:
        print(f"MODL v7: {result['node_count']} nodes; next chunk {result['next_chunk']}")
        for node in result["nodes"]:
            print(f"{node['index']:3d} {node['name']} <- {node['parent']}")


if __name__ == "__main__":
    main()
