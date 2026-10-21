"""Bake a MetaHuman face's ARKit-52 poses into morph-target deltas.

    python3 bake_arkit.py <head dump stem> <head eval stem> <out.npz>

Inputs are `mhdna dump` and `mhdna eval` output. For every pose, RigLogic's joint deltas are
composed onto the neutral joints the way UE's AnimNode_RigLogic does (T = N + D, R = N * D,
S = N + D), its blend shape weights are applied to the bind mesh, and the result is linear-blend
skinned. A morph is posed minus bind, so it carries the joint motion and the correctives together:
the engine plays the face from 52 weights alone and the face joints never animate.

MouthClose is ARKit's lips-together-while-the-jaw-is-open corrective, so it is baked as
(JawOpen + MouthClose) - JawOpen.
"""
import json
import sys

import numpy as np


def load(stem):
    j = json.load(open(stem + ".json"))
    raw = np.memmap(stem + ".bin", dtype=np.uint8, mode="r")

    def arr(name):
        s = j["arrays"][name]
        x = np.frombuffer(raw, dtype=s["dtype"], count=s["count"], offset=s["offset"])
        return x.reshape(-1, s["cols"]) if s["cols"] > 1 else x

    return j, arr


def quat_to_mat(q):
    x, y, z, w = q[:, 0], q[:, 1], q[:, 2], q[:, 3]
    m = np.empty((len(q), 3, 3))
    m[:, 0, 0] = 1 - 2 * (y * y + z * z)
    m[:, 0, 1] = 2 * (x * y - z * w)
    m[:, 0, 2] = 2 * (x * z + y * w)
    m[:, 1, 0] = 2 * (x * y + z * w)
    m[:, 1, 1] = 1 - 2 * (x * x + z * z)
    m[:, 1, 2] = 2 * (y * z - x * w)
    m[:, 2, 0] = 2 * (x * z - y * w)
    m[:, 2, 1] = 2 * (y * z + x * w)
    m[:, 2, 2] = 1 - 2 * (x * x + y * y)
    return m


def quat_mul(a, b):
    ax, ay, az, aw = a.T
    bx, by, bz, bw = b.T
    return np.stack([aw * bx + ax * bw + ay * bz - az * by,
                     aw * by - ax * bz + ay * bw + az * bx,
                     aw * bz + ax * by - ay * bx + az * bw,
                     aw * bw - ax * bx - ay * by - az * bz], axis=1)


def world_matrices(values, parents):
    """values: [J, 10] local (t, q xyzw, s). Returns [J, 4, 4] world matrices (column vectors)."""
    local = np.zeros((len(values), 4, 4))
    local[:, :3, :3] = quat_to_mat(values[:, 3:7]) * values[:, None, 7:10]
    local[:, :3, 3] = values[:, :3]
    local[:, 3, 3] = 1
    world = np.empty_like(local)
    for j, p in enumerate(parents):
        world[j] = local[j] if p == j or p >= len(values) else world[p] @ local[j]
    return world


def pose_values(neutral, delta):
    out = neutral.copy()
    out[:, :3] += delta[:, :3]
    out[:, 3:7] = quat_mul(neutral[:, 3:7], delta[:, 3:7])
    out[:, 7:10] += delta[:, 7:10]
    return out


def main():
    dj, da = load(sys.argv[1])
    ej, ea = load(sys.argv[2])
    out_path = sys.argv[3]
    parents = da("joint_parents").astype(np.int64)
    J = len(parents)
    neutral = ea("neutral_joint_values")[0].reshape(J, 10).astype(np.float64)
    deltas = ea("joint_outputs").reshape(len(ej["poses"]), J, 10).astype(np.float64)
    shapes = ea("blend_shape_outputs").astype(np.float64)
    poses = ej["poses"]

    w0_inv = np.linalg.inv(world_matrices(neutral, parents))
    skinning = {p: world_matrices(pose_values(neutral, deltas[i]), parents) @ w0_inv for i, p in enumerate(poses)}

    result = {"pose_names": np.array([p for p in poses if p != "neutral" and p != "MouthClose"] + ["MouthClose"])}
    report = []
    for mesh in dj["meshes"]:
        P = da(mesh + "/positions").astype(np.float64)
        counts = da(mesh + "/skin_counts").astype(np.int64)
        joints = da(mesh + "/skin_joints").astype(np.int64)
        weights = da(mesh + "/skin_weights").astype(np.float64)
        owner = np.repeat(np.arange(len(counts)), counts)
        bs_channel = da(mesh + "/bs_channel").astype(np.int64)
        bs_counts = da(mesh + "/bs_counts").astype(np.int64)
        bs_verts = da(mesh + "/bs_vertices").astype(np.int64)
        bs_deltas = da(mesh + "/bs_deltas").astype(np.float64).reshape(-1, 3)
        bs_start = np.concatenate([[0], np.cumsum(bs_counts)])

        def deform(i, pose):
            V = P.copy()
            for t, c in enumerate(bs_channel):
                w = shapes[i, c]
                if w != 0.0:
                    s, e = bs_start[t], bs_start[t + 1]
                    np.add.at(V, bs_verts[s:e], w * bs_deltas[s:e])
            M = skinning[pose][joints]
            Vh = np.concatenate([V[owner], np.ones((len(owner), 1))], axis=1)
            moved = np.einsum("nij,nj->ni", M, Vh)[:, :3] * weights[:, None]
            out = np.zeros_like(P)
            np.add.at(out, owner, moved)
            return out

        base = deform(poses.index("neutral"), "neutral")
        err = float(np.abs(base - P).max())
        per_pose = {}
        for i, pose in enumerate(poses):
            if pose == "neutral":
                continue
            per_pose[pose] = deform(i, pose) - base
        if "MouthClose" in per_pose and "JawOpen" in per_pose:
            per_pose["MouthClose"] = per_pose["MouthClose"] - per_pose["JawOpen"]
        stack = np.stack([per_pose[str(p)] for p in result["pose_names"]]).astype(np.float32)
        result[mesh] = stack
        result[mesh + "/positions"] = P.astype(np.float32)
        moved = {str(p): float(np.linalg.norm(stack[k], axis=1).max()) for k, p in enumerate(result["pose_names"])}
        report.append((mesh, err, moved))

    np.savez_compressed(out_path, **result)
    for mesh, err, moved in report:
        top = sorted(moved.items(), key=lambda kv: -kv[1])[:4]
        print(f"{mesh:22s} neutral err {err:.2e} cm   largest: " + ", ".join(f"{k} {v:.2f}" for k, v in top))
    print("wrote", out_path)


main()
