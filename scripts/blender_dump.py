"""Dump every realized mesh instance (depsgraph-expanded, so collection-instance
buildings are included) to JSON, in glTF axis convention, for comparison against
the baked .bsn / source glb.

Run from Blender: Scripting tab -> open this file -> Run Script, OR:
    blender your.blend --background --python scripts/blender_dump.py

Writes OUT below. Adjust OUT if your repo lives elsewhere.
"""

import bpy
import json
import mathutils

OUT = "/mnt/code/p/solari_files/blender_dump.json"

# Blender is Z-up, glTF is Y-up. A point (x, y, z)_blender -> (x, z, -y)_gltf.
def to_gltf_pt(v):
    return [round(v.x, 5), round(v.z, 5), round(-v.y, 5)]

# Axis-aligned sizes: swap Y/Z (sign-independent).
def to_gltf_dims(dx, dy, dz):
    return [round(dx, 5), round(dz, 5), round(dy, 5)]

def world_bbox(obj, mw):
    cs = [mw @ mathutils.Vector(c) for c in obj.bound_box]
    xs = [c.x for c in cs]; ys = [c.y for c in cs]; zs = [c.z for c in cs]
    mn = mathutils.Vector((min(xs), min(ys), min(zs)))
    mx = mathutils.Vector((max(xs), max(ys), max(zs)))
    center = (mn + mx) / 2.0
    return center, (mx.x - mn.x, mx.y - mn.y, mx.z - mn.z)

def shear_metric(m3):
    # How far the (scale-normalized) basis is from orthogonal. 0 == clean rotation*scale.
    cols = [mathutils.Vector((m3[0][i], m3[1][i], m3[2][i])) for i in range(3)]
    ncols = [c.normalized() if c.length > 1e-9 else c for c in cols]
    off = max(abs(ncols[a].dot(ncols[b])) for a, b in ((0, 1), (0, 2), (1, 2)))
    return round(off, 4)

deps = bpy.context.evaluated_depsgraph_get()
records = []
for inst in deps.object_instances:
    ob = inst.object
    if ob is None or ob.type != 'MESH' or ob.data is None or len(ob.data.vertices) == 0:
        continue
    mw = inst.matrix_world.copy()
    loc, rot, scale = mw.decompose()
    m3 = mw.to_3x3()
    det = m3.determinant()
    sc = [round(scale.x, 5), round(scale.y, 5), round(scale.z, 5)]
    asc = [abs(s) for s in sc]
    nonuniform = round(max(asc) / max(min(asc), 1e-9), 4)
    center, dims = world_bbox(ob, mw)
    records.append({
        "name": ob.name,
        "data": ob.data.name,
        "verts": len(ob.data.vertices),
        "is_instance": bool(inst.is_instance),
        "instancer": inst.parent.name if inst.parent else None,
        "loc_gltf": to_gltf_pt(loc),
        "bbox_center_gltf": to_gltf_pt(center),
        "dims_gltf": to_gltf_dims(*dims),
        "scale": sc,
        "rot_wxyz": [round(rot.w, 5), round(rot.x, 5), round(rot.y, 5), round(rot.z, 5)],
        "det_sign": 1 if det >= 0 else -1,
        "nonuniform": nonuniform,
        "shear": shear_metric(m3),
    })

summary = {
    "unit_scale": bpy.context.scene.unit_settings.scale_length,
    "count": len(records),
    "mirrored": sum(1 for r in records if r["det_sign"] < 0),
    "nonuniform_gt_1pct": sum(1 for r in records if r["nonuniform"] > 1.01),
    "sheared_gt_1pct": sum(1 for r in records if r["shear"] > 0.01),
    "distinct_meshdata": len({r["data"] for r in records}),
}
with open(OUT, "w") as f:
    json.dump({"summary": summary, "instances": records}, f)
print("blender_dump:", summary)
print("wrote", len(records), "instances ->", OUT)
