#!/usr/bin/env python3
# UE's retarget (ue_dump_pose.py json) vs our .animclip posed through the .skn.ron the runtime
# uses, every bone at once. Per bone: the angle between the two world-space rotations FROM BIND,
# after mapping UE's basis onto aurora's (fitted on the bind joint positions, reflection allowed).
#   compare_pose.py UE_JSON UE_CLIP SKN_RON ANIMCLIP [--top N] [--bones a,b,..]
import argparse
import json
import re
import struct

import numpy as np


def quat_mat(x, y, z, w):
    return np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])


def angle(a, b):
    return np.degrees(np.arccos(np.clip((np.trace(a.T @ b) - 1) / 2, -1, 1)))


def slerp(a, b, f):
    d = np.dot(a, b)
    if d < 0:
        b, d = -b, -d
    if d > 0.9995:
        q = a + f * (b - a)
    else:
        th = np.arccos(d)
        q = (np.sin((1 - f) * th) * a + np.sin(f * th) * b) / np.sin(th)
    return q / np.linalg.norm(q)


def load_skeleton(path):
    num = r"\(\s*([-\d.eE+]+),\s*([-\d.eE+]+),\s*([-\d.eE+]+)(?:,\s*([-\d.eE+]+))?,?\s*\)"
    pat = re.compile(r"path:\s*\[(.*?)\],\s*local:\s*\(\s*translation:\s*" + num + r",\s*rotation:\s*" + num, re.S)
    bones = []
    for m in pat.finditer(open(path).read()):
        p = tuple(s.strip().strip('"') for s in m.group(1).split(",") if s.strip())
        t = np.array([float(m.group(i)) for i in (2, 3, 4)])
        r = np.array([float(m.group(i)) for i in (6, 7, 8, 9)])
        bones.append((p, t, r))
    return bones


def load_animclip(path):
    f = open(path, "rb").read()
    assert f[:8] == b"ANIMCLP\x01", path
    (n,), o = struct.unpack_from("<I", f, 8), 12
    tracks = {}
    for _ in range(n):
        (c,), o = struct.unpack_from("<H", f, o), o + 2
        parts = []
        for _ in range(c):
            (ln,), o = struct.unpack_from("<H", f, o), o + 2
            parts.append(f[o:o + ln].decode())
            o += ln
        mask, o = f[o], o + 1
        ch = {}
        for bit, w, key in ((1, 3, "t"), (2, 4, "r"), (4, 3, "s")):
            if mask & bit:
                (k,), o = struct.unpack_from("<I", f, o), o + 4
                times = np.frombuffer(f, "<f4", k, o)
                o += 4 * k
                vals = np.frombuffer(f, "<f4", k * w, o).reshape(k, w)
                o += 4 * k * w
                ch[key] = (times, vals)
        tracks[tuple(parts)] = ch
    return tracks


def sample(ch, t, quat):
    times, vals = ch
    if t <= times[0] or len(times) == 1:
        return vals[0].astype(float)
    if t >= times[-1]:
        return vals[-1].astype(float)
    b = int(np.searchsorted(times, t, side="right"))
    a = b - 1
    f = (t - times[a]) / max(times[b] - times[a], 1e-9)
    va, vb = vals[a].astype(float), vals[b].astype(float)
    return slerp(va, vb, f) if quat else va + f * (vb - va)


def pose(bones, tracks, t):
    world = {}
    for p, lt, lr in bones:
        ch = tracks.get(p, {}) if tracks else {}
        if "t" in ch:
            lt = sample(ch["t"], t, False)
        if "r" in ch:
            lr = sample(ch["r"], t, True)
        r = quat_mat(*lr)
        parent = world.get(p[:-1])
        world[p] = (parent[0] + parent[1] @ lt, parent[1] @ r) if parent else (lt, r)
    return {p[-1].lower(): v for p, v in world.items()}, {p[-1].lower() for p in (tracks or {})}


def ue_frame(d):
    return {n.lower(): (np.array(v[:3]) * 0.01, quat_mat(*v[3:])) for n, v in d.items()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ue_json")
    ap.add_argument("ue_clip")
    ap.add_argument("skn")
    ap.add_argument("animclip")
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--bones", default="pelvis,spine_01,spine_02,spine_03,spine_04,spine_05,neck_01,neck_02,head,"
                    "clavicle_l,upperarm_l,lowerarm_l,hand_l,clavicle_r,upperarm_r,lowerarm_r,hand_r,thigh_l,calf_l,foot_l")
    a = ap.parse_args()

    clip = json.load(open(a.ue_json))[a.ue_clip]
    bones = load_skeleton(a.skn)
    tracks = load_animclip(a.animclip)
    aur_bind, _ = pose(bones, None, 0)
    ue_bind = ue_frame(clip["ref"])
    common = [n for n in ue_bind if n in aur_bind]

    # Basis: aurora = s * M @ ue (about each rig's pelvis), M orthogonal, det free.
    P = np.array([ue_bind[n][0] - ue_bind["pelvis"][0] for n in common])
    Q = np.array([aur_bind[n][0] - aur_bind["pelvis"][0] for n in common])
    u, _, vt = np.linalg.svd(Q.T @ P)
    M = u @ vt
    fit = np.linalg.norm(Q - P @ M.T, axis=1)
    print(f"{len(common)} common bones; basis det {np.linalg.det(M):+.0f}, bind position residual "
          f"mean {fit.mean() * 100:.2f} cm max {fit.max() * 100:.2f} cm ({common[int(fit.argmax())]})")
    print("M =", np.round(M, 3).tolist())

    for key, frame in clip["frames"].items():
        t = float(key)
        ue = ue_frame(frame)
        aur, keyed = pose(bones, tracks, t)
        rows = []
        for n in common:
            d_ue = M @ (ue[n][1] @ ue_bind[n][1].T) @ M.T
            d_aur = aur[n][1] @ aur_bind[n][1].T
            rows.append((n, angle(d_ue, d_aur), angle(np.eye(3), d_ue), angle(np.eye(3), d_aur), n in keyed))
        by = {r[0]: r for r in rows}
        print(f"\n== {a.ue_clip} t={t:.3f}s  (err = angle between UE and aurora world rotation-from-bind; "
              f"ue/aur = each one's own angle from bind; * = keyed in the .animclip)")
        print(f"{'bone':<22}{'err':>7}{'ue':>7}{'aur':>7}")
        for n in a.bones.split(","):
            if n in by:
                r = by[n]
                print(f"{n:<22}{r[1]:7.1f}{r[2]:7.1f}{r[3]:7.1f} {'*' if r[4] else ''}")
        print(f"-- worst {a.top}:")
        for r in sorted(rows, key=lambda r: -r[1])[:a.top]:
            print(f"{r[0]:<22}{r[1]:7.1f}{r[2]:7.1f}{r[3]:7.1f} {'*' if r[4] else ''}")


if __name__ == "__main__":
    main()
