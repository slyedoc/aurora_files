#!/usr/bin/env python3
# UE's own retarget (ue_dump_pose.py json, FPS=30, onto the imported winston.glb) -> .animclip
# + .anim.ron on the same skeleton's .skn.ron. Both sides share one bind, so each bone's frame
# convention is the constant K = (M Wu_bind M^T)^T Wa_bind and nothing is re-derived.
#   ue_to_animclip.py UE_JSON SKN_RON OUT_DIR [--prefix W_]
import argparse
import json
import os
import struct
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from compare_pose import load_skeleton, quat_mat  # noqa: E402


def mat_quat(m):
    w = np.sqrt(max(0.0, 1 + m[0, 0] + m[1, 1] + m[2, 2])) / 2
    x = np.sqrt(max(0.0, 1 + m[0, 0] - m[1, 1] - m[2, 2])) / 2
    y = np.sqrt(max(0.0, 1 - m[0, 0] + m[1, 1] - m[2, 2])) / 2
    z = np.sqrt(max(0.0, 1 - m[0, 0] - m[1, 1] + m[2, 2])) / 2
    x = np.copysign(x, m[2, 1] - m[1, 2])
    y = np.copysign(y, m[0, 2] - m[2, 0])
    z = np.copysign(z, m[1, 0] - m[0, 1])
    q = np.array([x, y, z, w])
    return q / np.linalg.norm(q)


def write_animclip(path, targets):
    with open(path, "wb") as f:
        f.write(b"ANIMCLP\x01" + struct.pack("<I", len(targets)))
        for p, times, rot, trans in targets:
            f.write(struct.pack("<H", len(p)))
            for part in p:
                b = part.encode()
                f.write(struct.pack("<H", len(b)) + b)
            f.write(bytes([2 | (1 if trans is not None else 0)]))
            for vals in ([trans] if trans is not None else []) + [rot]:
                f.write(struct.pack("<I", len(times)))
                f.write(np.asarray(times, "<f4").tobytes())
                f.write(np.asarray(vals, "<f4").tobytes())


def asset_path(p):
    parts = os.path.abspath(p).split(os.sep)
    if "assets" not in parts:
        return parts[-1]
    i = len(parts) - 1 - parts[::-1].index("assets")
    return "/".join(parts[i + 1:])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ue_json")
    ap.add_argument("skn")
    ap.add_argument("out")
    ap.add_argument("--prefix", default="W_")
    a = ap.parse_args()

    bones = load_skeleton(a.skn)
    bind = {}
    for p, lt, lr in bones:
        parent = bind.get(p[:-1])
        r = quat_mat(*lr)
        bind[p] = (parent[0] + parent[1] @ lt, parent[1] @ r) if parent else (lt, r)
    by_name = {p[-1].lower(): p for p, _, _ in bones}
    local_bind = {p: (lt, lr) for p, lt, lr in bones}

    os.makedirs(a.out, exist_ok=True)
    for name, clip in json.load(open(a.ue_json)).items():
        ue_ref = {n.lower(): v for n, v in clip["ref"].items()}
        common = [n for n in ue_ref if n in by_name]
        P = np.array([ue_ref[n][:3] for n in common]) * 0.01
        Q = np.array([bind[by_name[n]][0] for n in common])
        pc, qc = P.mean(0), Q.mean(0)
        u, _, vt = np.linalg.svd((Q - qc).T @ (P - pc))
        M = u @ vt
        res = np.linalg.norm((Q - qc) - (P - pc) @ M.T, axis=1)
        K = {n: (M @ quat_mat(*ue_ref[n][3:]) @ M.T).T @ bind[by_name[n]][1] for n in common}

        keys = sorted(clip["frames"], key=float)
        times = [float(k) for k in keys]
        world = []
        for k in keys:
            fr = {n.lower(): v for n, v in clip["frames"][k].items()}
            w = {}
            for n in common:
                v = fr[n]
                w[by_name[n]] = (qc + M @ (np.array(v[:3]) * 0.01 - pc), M @ quat_mat(*v[3:]) @ M.T @ K[n])
            world.append(w)

        targets = []
        for n in common:
            p = by_name[n]
            rots, trans = [], []
            for w in world:
                pw = w.get(p[:-1], bind.get(p[:-1], (np.zeros(3), np.eye(3))))
                rots.append(mat_quat(pw[1].T @ w[p][1]))
                trans.append(pw[1].T @ (w[p][0] - pw[0]))
            rots = np.array(rots)
            for i in range(1, len(rots)):
                if np.dot(rots[i], rots[i - 1]) < 0:
                    rots[i] = -rots[i]
            trans = np.array(trans)
            moves = np.abs(trans - local_bind[p][0]).max() > 1e-3
            targets.append((p, times, rots, trans if moves else None))

        stem = name[len(a.prefix):] if name.startswith(a.prefix) else name
        out = os.path.join(a.out, stem + ".animclip")
        write_animclip(out, targets)
        with open(os.path.join(a.out, stem + ".anim.ron"), "w") as f:
            f.write(f'(\n    source: AnimClip(\n        path: "{asset_path(out)}",\n    ),\n'
                    f'    skeleton: "{asset_path(a.skn)}",\n    event_tracks: {{}},\n)')
        moved = [t[0][-1] for t in targets if t[3] is not None]
        print(f"{stem}: {len(times)} frames, {len(targets)} bones, bind residual {res.max() * 100:.3f} cm, "
              f"translation keyed on {moved} -> {out}")


if __name__ == "__main__":
    main()
