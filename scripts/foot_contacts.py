"""Find where each foot plants in a skinned glb's clips and write `footstep` event tracks.

A foot plants when it drops into the lowest band of its cycle (within --band of its minimum
height). Forward kinematics over the glb's own node tree, sampled at --samples per clip; each
contact becomes a StringId event on a "footstep" track of the clip's baked `<stem>.anim.ron`
(written by prop_import --hierarchy), which jackdaw's timeline edits from there.

    python3 scripts/foot_contacts.py raw/wow_human/humanmale.glb \\
        ../zero/assets/characters/human_male \\
        --feet bone_FootL=footstep_l,bone_FootR=footstep_r \\
        --clips "Walk,Run,Walkbackwards,Sprint"

A wrapper that already has event tracks is left alone unless --replace.
"""

import argparse
import json
import re
import struct
import sys
import uuid
from pathlib import Path

import numpy as np

COMPONENT = {5126: "f", 5123: "H", 5121: "B", 5125: "I"}
WIDTH = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


def load_glb(path):
    data = Path(path).read_bytes()
    json_len = struct.unpack("<I", data[12:16])[0]
    doc = json.loads(data[20 : 20 + json_len])
    bin_len = struct.unpack("<I", data[20 + json_len : 24 + json_len])[0]
    blob = data[28 + json_len : 28 + json_len + bin_len]
    return doc, blob


def accessor(doc, blob, index):
    acc = doc["accessors"][index]
    view = doc["bufferViews"][acc["bufferView"]]
    offset = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    width = WIDTH[acc["type"]]
    dtype = np.dtype("<" + COMPONENT[acc["componentType"]])
    values = np.frombuffer(blob, dtype=dtype, count=acc["count"] * width, offset=offset)
    return values.reshape(acc["count"], width).astype(np.float64)


def quat_mul(a, b):
    x1, y1, z1, w1 = a
    x2, y2, z2, w2 = b
    return np.array(
        [
            w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
            w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
            w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
            w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
        ]
    )


def quat_rotate(q, v):
    u, w = q[:3], q[3]
    return 2 * np.dot(u, v) * u + (w * w - np.dot(u, u)) * v + 2 * w * np.cross(u, v)


def nlerp(a, b, t):
    if np.dot(a, b) < 0:
        b = -b
    r = a * (1 - t) + b * t
    return r / np.linalg.norm(r)


def clip_stem(name):
    """Mirror of aurora_bsn::anim::clip_stem: wow.export `Stand (ID 0 variation 1)` -> stand_v1."""
    m = re.fullmatch(r"(.*) \(ID \d+ variation (\d+)\)", name)
    if not m:
        return name.lower()
    snake = re.sub(r"(?<=[a-z0-9])([A-Z])", r"_\1", m.group(1)).lower()
    return snake if m.group(2) == "0" else f"{snake}_v{m.group(2)}"


class Clip:
    def __init__(self, doc, blob, anim):
        self.channels = []
        self.duration = 0.0
        for ch in anim["channels"]:
            path = ch["target"]["path"]
            if path not in ("translation", "rotation", "scale"):
                continue
            sampler = anim["samplers"][ch["sampler"]]
            times = accessor(doc, blob, sampler["input"])[:, 0]
            values = accessor(doc, blob, sampler["output"])
            self.channels.append((ch["target"]["node"], path, times, values))
            self.duration = max(self.duration, times[-1])

    def sample(self, t):
        out = {}
        for node, path, times, values in self.channels:
            k = np.searchsorted(times, t)
            if k <= 0:
                v = values[0]
            elif k >= len(times):
                v = values[-1]
            else:
                f = (t - times[k - 1]) / (times[k] - times[k - 1])
                v = nlerp(values[k - 1], values[k], f) if path == "rotation" else values[k - 1] * (1 - f) + values[k] * f
            out.setdefault(node, {})[path] = v
        return out


def world_position(nodes, parent, pose, i):
    node = nodes[i]
    own = pose.get(i, {})
    t = np.array(own.get("translation", node.get("translation", [0, 0, 0])), float)
    r = np.array(own.get("rotation", node.get("rotation", [0, 0, 0, 1])), float)
    if i in parent:
        pt, pr = world_position(nodes, parent, pose, parent[i])
        return pt + quat_rotate(pr, t), quat_mul(pr, r)
    return t, r


def contacts(heights, band):
    """Sample indices where the foot enters its lowest band, treating the clip as a cycle."""
    low = heights < heights.min() + band
    return [i for i in range(len(low)) if low[i] and not low[i - 1]]


def track_ron(events, window):
    lines = ['        "footstep": (', '            name: "footstep",', "            events: ["]
    for ident, name, start in events:
        lines += [
            "                (",
            f'                    id: "{ident}",',
            "                    value: (",
            f'                        event: StringId("{name}"),',
            f"                        start_time: {start:.4f},",
            f"                        end_time: {start + window:.4f},",
            "                    ),",
            "                ),",
        ]
    lines += ["            ],", "        ),"]
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("glb")
    ap.add_argument("out_dir", help="directory holding the baked <stem>.anim.ron wrappers")
    ap.add_argument("--feet", required=True, help="bone=event pairs, comma separated")
    ap.add_argument("--clips", required=True, help="glb clip names without the wow.export suffix")
    ap.add_argument("--band", type=float, default=0.03, help="height above the minimum that counts as planted (glb units)")
    ap.add_argument("--window", type=float, default=0.1, help="event length in seconds")
    ap.add_argument("--samples", type=int, default=240)
    ap.add_argument("--replace", action="store_true", help="overwrite existing event tracks")
    args = ap.parse_args()

    doc, blob = load_glb(args.glb)
    nodes = doc["nodes"]
    parent = {c: i for i, n in enumerate(nodes) for c in n.get("children", [])}
    index = {n.get("name"): i for i, n in enumerate(nodes)}
    feet = [pair.split("=") for pair in args.feet.split(",")]
    by_base = {}
    for anim in doc["animations"]:
        by_base.setdefault(anim["name"].split(" (ID ")[0], anim)
        by_base.setdefault(anim["name"], anim)

    failed = False
    for name in args.clips.split(","):
        anim = by_base.get(name)
        if anim is None:
            print(f"{name}: no such clip", file=sys.stderr)
            failed = True
            continue
        stem = clip_stem(anim["name"])
        wrapper = Path(args.out_dir) / f"{stem}.anim.ron"
        text = wrapper.read_text()
        if not re.search(r"event_tracks: \{\s*\}", text) and not args.replace:
            print(f"{wrapper}: has event tracks, skipped (--replace to overwrite)")
            continue
        clip = Clip(doc, blob, anim)
        times = np.linspace(0, clip.duration, args.samples, endpoint=False)
        poses = [clip.sample(t) for t in times]
        events = []
        for bone, event in feet:
            heights = np.array([world_position(nodes, parent, p, index[bone])[0][1] for p in poses])
            for i in contacts(heights, args.band):
                ident = uuid.uuid5(uuid.NAMESPACE_URL, f"footstep/{stem}/{event}/{i}")
                events.append((ident, event, float(times[i])))
        events.sort(key=lambda e: e[2])
        body = "event_tracks: {\n" + track_ron(events, args.window) + "\n    },"
        text = re.sub(r"event_tracks: \{.*\},?", lambda _: body, text, count=1, flags=re.S)
        wrapper.write_text(text)
        summary = ", ".join(f"{e} {t:.3f}" for _, e, t in events)
        print(f"{stem} ({clip.duration:.3f} s): {summary}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
