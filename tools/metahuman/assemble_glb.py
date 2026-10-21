"""Assemble a MetaHuman into a skinned glb with ARKit-52 shape keys, in Blender.

    blender -b --python assemble_glb.py -- <raw dir> <Maps dir> <out.glb>

<raw dir> holds `mhdna` output for the body and head (body.json/bin, body_eval.json/bin,
head.json/bin) and `bake_arkit.py`'s head_arkit.npz. The body skeleton is the armature; the face
meshes skin to it with every face joint's weight moved to its nearest body-skeleton ancestor,
because the face moves through the 52 shape keys (joint motion and correctives baked in), not
through its joints. DNA is centimetres, x left / y up / z front; Blender gets metres, Z up.
"""
import json
import os
import sys

import bmesh
import bpy
import numpy as np
from mathutils import Matrix, Quaternion, Vector
from mathutils.bvhtree import BVHTree

ARKIT = [
    "EyeBlinkLeft", "EyeLookDownLeft", "EyeLookInLeft", "EyeLookOutLeft", "EyeLookUpLeft", "EyeSquintLeft",
    "EyeWideLeft", "EyeBlinkRight", "EyeLookDownRight", "EyeLookInRight", "EyeLookOutRight", "EyeLookUpRight",
    "EyeSquintRight", "EyeWideRight", "JawForward", "JawLeft", "JawRight", "JawOpen", "MouthClose", "MouthFunnel",
    "MouthPucker", "MouthLeft", "MouthRight", "MouthSmileLeft", "MouthSmileRight", "MouthFrownLeft",
    "MouthFrownRight", "MouthDimpleLeft", "MouthDimpleRight", "MouthStretchLeft", "MouthStretchRight",
    "MouthRollLower", "MouthRollUpper", "MouthShrugLower", "MouthShrugUpper", "MouthPressLeft", "MouthPressRight",
    "MouthLowerDownLeft", "MouthLowerDownRight", "MouthUpperUpLeft", "MouthUpperUpRight", "BrowDownLeft",
    "BrowDownRight", "BrowInnerUp", "BrowOuterUpLeft", "BrowOuterUpRight", "CheekPuff", "CheekSquintLeft",
    "CheekSquintRight", "NoseSneerLeft", "NoseSneerRight", "TongueOut",
]
# Face meshes that ship; the eye shell, eye edge and saliva are translucent film the tracer
# would draw as opaque skin.
FACE_MESHES = {
    "head_lod0_mesh": "Head",
    "teeth_lod0_mesh": "Teeth",
    "eyeLeft_lod0_mesh": "Eyes",
    "eyeRight_lod0_mesh": "Eyes",
    "eyelashes_lod0_mesh": "Eyelashes",
    "cartilage_lod0_mesh": "Head",
}
MATERIALS = {
    "Body": ("Body_Basecolor.png", "Body_Normal.png"),
    "Head": ("Head_Basecolor.png", "Head_Normal.png"),
    "Teeth": ("Teeth_Color.png", "Teeth_Normal.png"),
    "Eyes": ("Eyes_Color.png", "Eyes_Normal.png"),
    "Eyelashes": ("Eyelashes_Color.png", None),
}


def to_blender(p):
    """DNA cm (x left, y up, z front) -> Blender m (Z up, -Y front), as glTF import maps it."""
    p = np.asarray(p, dtype=np.float64) * 0.01
    return np.stack([p[..., 0], -p[..., 2], p[..., 1]], axis=-1)


def load(stem):
    j = json.load(open(stem + ".json"))
    raw = np.fromfile(stem + ".bin", dtype=np.uint8)

    def arr(name):
        s = j["arrays"][name]
        x = np.frombuffer(raw, dtype=s["dtype"], count=s["count"], offset=s["offset"])
        return x.reshape(-1, s["cols"]) if s["cols"] > 1 else x

    return j, arr


def world_matrices(values, parents):
    """[J, 10] local (t, quat xyzw, s) -> world 4x4 in DNA space."""
    out = []
    for j, v in enumerate(values):
        q = Quaternion((v[6], v[3], v[4], v[5]))
        local = Matrix.Translation(Vector(v[:3])) @ q.to_matrix().to_4x4() @ Matrix.Diagonal((*v[7:10], 1.0))
        p = parents[j]
        out.append(out[p] @ local if p != j and p < len(values) else local)
    return out


AXES = Matrix(((0.01, 0, 0, 0), (0, 0, -0.01, 0), (0, 0.01, 0, 0), (0, 0, 0, 1)))


def build_armature(names, parents, worlds):
    arm_data = bpy.data.armatures.new("Winston_Skeleton")
    arm = bpy.data.objects.new("Winston", arm_data)
    bpy.context.scene.collection.objects.link(arm)
    bpy.context.view_layer.objects.active = arm
    bpy.ops.object.mode_set(mode="EDIT")
    bones = []
    for j, name in enumerate(names):
        b = arm_data.edit_bones.new(name)
        m = AXES @ worlds[j]
        head = m.translation
        b.head = head
        b.tail = head + (m.to_3x3() @ Vector((0.0, 0.03, 0.0))) * 0.01
        if (b.tail - b.head).length < 1e-5:
            b.tail = head + Vector((0, 0, 0.01))
        bones.append(b)
    for j, p in enumerate(parents):
        if p != j and p < len(names):
            bones[j].parent = bones[p]
    bpy.ops.object.mode_set(mode="OBJECT")
    return arm


def material(name, maps_dir):
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = nt.nodes["Principled BSDF"]
    base, normal = MATERIALS[name]
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = bpy.data.images.load(os.path.join(maps_dir, base))
    nt.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    if name == "Eyelashes":
        # Alpha through a Greater Than is how Blender's glTF exporter writes alphaMode MASK
        # (cutoff = the threshold); a straight link exports BLEND, which the bake treats as opaque.
        cut = nt.nodes.new("ShaderNodeMath")
        cut.operation = "GREATER_THAN"
        cut.inputs[1].default_value = 0.4
        nt.links.new(tex.outputs["Alpha"], cut.inputs[0])
        nt.links.new(cut.outputs[0], bsdf.inputs["Alpha"])
    if normal:
        ntex = nt.nodes.new("ShaderNodeTexImage")
        ntex.image = bpy.data.images.load(os.path.join(maps_dir, normal))
        ntex.image.colorspace_settings.name = "Non-Color"
        nmap = nt.nodes.new("ShaderNodeNormalMap")
        nt.links.new(ntex.outputs["Color"], nmap.inputs["Color"])
        nt.links.new(nmap.outputs["Normal"], bsdf.inputs["Normal"])
    bsdf.inputs["Roughness"].default_value = {"Eyes": 0.1, "Teeth": 0.35}.get(name, 0.55)
    return mat


def build_mesh(name, arr, mesh, joint_names, joint_map, mat, arm, uv_shift=0.0):
    P = to_blender(arr(mesh + "/positions"))
    uvs = arr(mesh + "/uvs").astype(np.float64)
    layouts = arr(mesh + "/layouts")
    face_layouts = arr(mesh + "/face_layouts")
    sizes = arr(mesh + "/face_sizes")
    me = bpy.data.meshes.new(name)
    bm = bmesh.new()
    verts = [bm.verts.new(p) for p in P]
    bm.verts.ensure_lookup_table()
    uv_layer = bm.loops.layers.uv.new("UVMap")
    start = 0
    for n in sizes:
        lay = face_layouts[start:start + n]
        start += n
        try:
            f = bm.faces.new([verts[layouts[l][0]] for l in lay])
        except ValueError:
            continue
        for loop, l in zip(f.loops, lay):
            u, v = uvs[layouts[l][1]]
            loop[uv_layer].uv = (u - uv_shift, v)
    bm.to_mesh(me)
    bm.free()
    me.materials.append(mat)
    for poly in me.polygons:
        poly.use_smooth = True
    ob = bpy.data.objects.new(name, me)
    bpy.context.scene.collection.objects.link(ob)
    ob.parent = arm
    mod = ob.modifiers.new("Armature", "ARMATURE")
    mod.object = arm

    counts = arr(mesh + "/skin_counts").astype(np.int64)
    joints = arr(mesh + "/skin_joints").astype(np.int64)
    weights = arr(mesh + "/skin_weights").astype(np.float64)
    groups = {}
    k = 0
    for v, c in enumerate(counts):
        acc = {}
        for j, w in zip(joints[k:k + c], weights[k:k + c]):
            target = joint_map[joint_names[j]]
            acc[target] = acc.get(target, 0.0) + float(w)
        k += c
        for target, w in acc.items():
            g = groups.get(target)
            if g is None:
                g = groups[target] = ob.vertex_groups.new(name=target)
            g.add([v], w, "REPLACE")
    return ob


# name: (base colour, roughness, offset from the skin in m, smoothing passes)
SUIT = (0.012, 0.016, 0.032)
GARMENTS = {
    "Shirt": ((0.30, 0.42, 0.62), 0.75, 0.012, 16),
    "Jacket": (SUIT, 0.7, 0.026, 30),
    "Trousers": (SUIT, 0.75, 0.016, 24),
    "Shoes": ((0.015, 0.015, 0.016), 0.3, 0.010, 8),
}
# Loose fit: below these radii (m, at each joint of the chain) a sleeve or leg is pushed out.
LOOSE = {
    "Shirt": (("upperarm", "lowerarm", "hand"), (0.0, 0.048, 0.036)),
    "Jacket": (("upperarm", "lowerarm", "hand"), (0.0, 0.052, 0.042)),
    "Trousers": (("thigh", "calf", "foot"), (0.0, 0.088, 0.078)),
}
THICKNESS = 0.003
SKIN_GAP = 0.004


def bone_part(name):
    if "toe" in name or name.startswith(("foot", "ball", "ankle")):
        return "foot"
    if name.startswith(("hand", "pinky", "ring", "middle", "index", "thumb", "wrist")):
        return "hand"
    if name.startswith(("neck", "head")):
        return "neck"
    if name.startswith("lowerarm"):
        return "forearm"
    if name.startswith(("upperarm", "clavicle")):
        return "arm"
    if name.startswith(("thigh", "calf")):
        return "leg"
    return "trunk"


class Cut:
    """Where each garment ends, as planes (point, normal; kept side is normal . (p - point) < 0)."""

    def __init__(self, arm):
        self.head = {b.name: Vector(b.head_local) for b in arm.data.bones}
        h = self.head
        self.waist = h["spine_01"].z + 0.03
        self.ankle = max(h["foot_l"].z, h["foot_r"].z)
        up = Vector((0, 0, 1))
        flat = lambda z: (Vector((0, 0, z)), up)  # noqa: E731
        # Front is -Y: the collar dips toward the chest.
        collar = (h["neck_01"] + Vector((0, 0, 0.045)), Vector((0, -0.3, 1)).normalized())
        lapel_collar = (collar[0] - Vector((0, 0, 0.012)), collar[1])
        cuffs, sleeves = {}, {}
        for side in "lr":
            axis = (h[f"hand_{side}"] - h[f"lowerarm_{side}"]).normalized()
            cuffs[side] = (h[f"hand_{side}"] - axis * 0.035, axis)
            sleeves[side] = (h[f"hand_{side}"] - axis * 0.05, axis)
        # The jacket's hem sits over the hips; its front opens in a V from the button to the collar.
        self.hem = h["pelvis"].z - 0.06
        self.button = self.waist + 0.06
        self.front = h["spine_03"].y
        self.lapels = [(Vector((0, 0, self.button)), Vector((-sx, 0, 0.24)).normalized()) for sx in (1, -1)]
        self.planes = {
            "Shirt": [flat(self.waist - 0.05), collar, cuffs["l"], cuffs["r"]],
            "Jacket": [flat(self.hem), lapel_collar, sleeves["l"], sleeves["r"], *self.lapels],
            "Trousers": [flat(self.waist + 0.01), flat(self.ankle + 0.03)],
            "Shoes": [flat(self.ankle + 0.05)],
        }
        self.collar, self.lapel_collar, self.cuffs, self.sleeves = collar, lapel_collar, cuffs, sleeves

    def keeps(self, name, part, c):
        below = lambda plane: (c - plane[0]).dot(plane[1]) < 0  # noqa: E731
        z = c.z
        if name == "Shirt":
            if part == "trunk":
                return z > self.waist - 0.05 and below(self.collar)
            if part in ("arm", "neck"):
                return below(self.collar)
            if part == "forearm":
                return below(self.cuffs["l" if c.x > 0 else "r"])
            return False
        if name == "Jacket":
            if c.y < self.front and c.z > self.button and abs(c.x) < (c.z - self.button) * 0.24:
                return False
            if part == "trunk":
                return z > self.hem and below(self.lapel_collar)
            if part == "leg":
                return z > self.hem
            if part in ("arm", "neck"):
                return below(self.lapel_collar)
            if part == "forearm":
                return below(self.sleeves["l" if c.x > 0 else "r"])
            return False
        if name == "Trousers":
            return (part == "trunk" and z < self.waist + 0.01) or (part == "leg" and z > self.ankle + 0.03)
        if name == "Shoes":
            return part == "foot" or (part == "leg" and z < self.ankle + 0.05)
        return False


def vert_parts(bm, names):
    dl = bm.verts.layers.deform.verify()
    return {v: bone_part(names[max(v[dl].items(), key=lambda kv: kv[1])[0]]) if v[dl] else "trunk" for v in bm.verts}


def face_part(f, parts):
    seen = [parts[v] for v in f.verts]
    return max(set(seen), key=seen.count)


def loosen(bm, cut, name, parts):
    chain, radii = LOOSE[name]
    want = {"Shirt": ("arm", "forearm"), "Jacket": ("arm", "forearm"), "Trousers": ("leg",)}[name]
    for v in bm.verts:
        if parts[v] not in want:
            continue
        side = "l" if v.co.x > 0 else "r"
        joints = [cut.head[f"{j}_{side}"] for j in chain]
        best = None
        for i in range(2):
            a, b = joints[i], joints[i + 1]
            seg = b - a
            t = max(0.0, min(1.0, (v.co - a).dot(seg) / seg.length_squared))
            radial = v.co - (a + seg * t)
            if best is None or radial.length < best[0].length:
                # The fit fades in from the shoulder or hip, so the trunk's cloth meets it smoothly.
                r = radii[i] + (radii[i + 1] - radii[i]) * t if i else radii[1] * min(1.0, t * 2.5)
                best = (radial, r)
        radial, r = best
        if 1e-6 < radial.length < r:
            v.co += radial.normalized() * (r - radial.length)


class Neck:
    """The head mesh's surface and skin weights, for a collar to snap to and move with."""

    def __init__(self, head):
        me = head.data
        self.co = [v.co.copy() for v in me.vertices]
        self.polys = [p.vertices[:] for p in me.polygons]
        self.tree = BVHTree.FromPolygons(self.co, self.polys)
        names = self.names = [g.name for g in head.vertex_groups]
        self.weights = [{names[g.group]: g.weight for g in v.groups} for v in me.vertices]

    def nearest(self, p):
        loc, _, face, _ = self.tree.find_nearest(p)
        vi = min(self.polys[face], key=lambda i: (self.co[i] - loc).length)
        return loc, self.weights[vi]


def grow_collar(bm, cut, neck, groups):
    """The body mesh stops at the neck seam (the head mesh carries the neck): extrude its edge up
    the neck, each ring snapped onto the neck's surface and skinned like it, so the collar has cloth
    for its plane to cut and moves with the neck when the head turns."""
    h = cut.head
    up = (h["head"] - h["neck_01"]).normalized()
    dl = bm.verts.layers.deform.verify()
    edges = [e for e in bm.edges if e.is_boundary]  # the body's only open edge is the neck seam
    for ring in range(4):
        out = bmesh.ops.extrude_edge_only(bm, edges=edges)["geom"]
        for v in (g for g in out if isinstance(g, bmesh.types.BMVert)):
            v.co, skin = neck.nearest(v.co + up * 0.018)
            # The first ring blends from the seam's weights into the neck's.
            mix = 0.5 if ring == 0 else 1.0
            blend = {k: w * (1 - mix) for k, w in v[dl].items()}
            for name, w in skin.items():
                blend[groups[name]] = blend.get(groups[name], 0.0) + w * mix
            v[dl].clear()
            for k, w in blend.items():
                v[dl][k] = w
        edges = [g for g in out if isinstance(g, bmesh.types.BMEdge) and g.is_boundary]
    # Extruded faces take an arbitrary winding; make them agree with the body they grow from.
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])


def garment(body, neck, name, cut, under):
    colour, roughness, offset, passes = GARMENTS[name]
    me = body.data.copy()
    me.name = name
    ob = bpy.data.objects.new(name, me)
    bpy.context.scene.collection.objects.link(ob)
    ob.parent = body.parent
    for g in body.vertex_groups:
        ob.vertex_groups.new(name=g.name)
    for n in neck.names:
        if n not in ob.vertex_groups:
            ob.vertex_groups.new(name=n)
    names = [g.name for g in ob.vertex_groups]
    bm = bmesh.new()
    bm.from_mesh(me)
    if name in ("Shirt", "Jacket"):
        grow_collar(bm, cut, neck, {n: i for i, n in enumerate(names)})
    for co, no in cut.planes[name]:
        bmesh.ops.bisect_plane(bm, geom=bm.verts[:] + bm.edges[:] + bm.faces[:], plane_co=co, plane_no=no)
    parts = vert_parts(bm, names)
    drop = [f for f in bm.faces if not cut.keeps(name, face_part(f, parts), f.calc_center_median())]
    bmesh.ops.delete(bm, geom=drop, context="FACES")
    # Under UE's pieces (the tee over the waistband) this cloth would only poke through; it stops
    # two rings in, so the two still overlap at the hem.
    bm.normal_update()
    hidden = erode({v for v in bm.verts if under(v.co, v.normal)})
    bmesh.ops.delete(bm, geom=[f for f in bm.faces if all(v in hidden for v in f.verts)], context="FACES")
    bmesh.ops.delete(bm, geom=[v for v in bm.verts if not v.link_faces], context="VERTS")
    parts = vert_parts(bm, names)
    bm.normal_update()
    skin = {v: (v.co.copy(), v.normal.copy()) for v in bm.verts}
    # Cloth drapes over anatomy: navel, nipples and muscle smooth out before it lifts off.
    inner = [v for v in bm.verts if not v.is_boundary]
    for _ in range(passes):
        bmesh.ops.smooth_vert(bm, verts=inner, factor=0.5, use_axis_x=True, use_axis_y=True, use_axis_z=True)
    if name in LOOSE:
        loosen(bm, cut, name, parts)
        for _ in range(4):
            bmesh.ops.smooth_vert(bm, verts=inner, factor=0.5, use_axis_x=True, use_axis_y=True, use_axis_z=True)
    # Smoothing shrinks anything convex (neck, belly); cloth may sink toward the skin but always
    # stays SKIN_GAP clear of it.
    for v, (co, n) in skin.items():
        inward = (v.co - co).dot(n) + offset - SKIN_GAP
        if inward < 0:
            v.co -= n * inward
    bm.normal_update()
    for v in bm.verts:
        v.co += v.normal * offset
    bm.to_mesh(me)
    bm.free()
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (*colour, 1.0)
    bsdf.inputs["Roughness"].default_value = roughness
    me.materials.clear()
    me.materials.append(mat)
    solid = ob.modifiers.new("Solidify", "SOLIDIFY")
    solid.thickness = THICKNESS
    solid.offset = -1.0
    solid.use_rim = True
    with bpy.context.temp_override(object=ob, active_object=ob, selected_objects=[ob]):
        bpy.ops.object.modifier_apply(modifier=solid.name)
    ob.modifiers.new("Armature", "ARMATURE").object = body.parent
    return ob


# UE's own outfit (MetaHuman Creator's default garment, fitted to Winston by its Cinematic build;
# ue_export_outfit.py) supplies the pieces named here, by material; the procedural garments
# above fill in the rest. The default garment is a tee and shorts: the shorts give way to trousers.
OUTFIT_PIECES = {"Shirt": (0.59, 0.59, 0.59)}
PROCEDURAL = ("Trousers", "Shoes")


def similarity(P, Q):
    """s, R, t minimising |s R P + t - Q| (Umeyama)."""
    pc, qc = P.mean(0), Q.mean(0)
    u, d, vt = np.linalg.svd((Q - qc).T @ (P - pc))
    S = np.eye(3)
    S[2, 2] = np.sign(np.linalg.det(u @ vt))
    R = u @ S @ vt
    s = (d * np.diag(S)).sum() / ((P - pc) ** 2).sum()
    return s, R, qc - s * R @ pc


def outfit_material(name, colour, tex_dir):
    """Base colour = UE's diffuse colour under the piece's AO; its normal map as is."""
    stem = os.path.join(tex_dir, f"DG_bodyShapeDE_{'Short' if name == 'Shorts' else name}")
    ao = bpy.data.images.load(stem + "_AO.png")
    ao.scale(2048, 2048)
    px = np.empty(2048 * 2048 * 4, np.float32)
    ao.pixels.foreach_get(px)
    px = px.reshape(-1, 4)
    base = bpy.data.images.new(f"{name}_BaseColor", 2048, 2048)
    out = np.ones_like(px)
    out[:, :3] = px[:, :1] * np.array(colour) ** (1 / 2.2)
    base.pixels.foreach_set(out.ravel())
    base.filepath_raw = stem + "_BaseColor.png"
    base.file_format = "PNG"
    base.save()
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = nt.nodes["Principled BSDF"]
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = base
    nt.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    ntex = nt.nodes.new("ShaderNodeTexImage")
    ntex.image = bpy.data.images.load(stem + "_Normal.png")
    ntex.image.colorspace_settings.name = "Non-Color"
    nmap = nt.nodes.new("ShaderNodeNormalMap")
    nt.links.new(ntex.outputs["Color"], nmap.inputs["Color"])
    nt.links.new(nmap.outputs["Normal"], bsdf.inputs["Normal"])
    bsdf.inputs["Roughness"].default_value = 0.85
    return mat


def import_outfit(path, tex_dir, arm):
    """UE's fitted outfit onto our armature: FBX axes and units are whatever they are, so the
    transform is FITTED from the two skeletons' bind joint positions rather than assumed."""
    before = set(bpy.data.objects)
    bpy.ops.import_scene.fbx(filepath=path)
    new = [o for o in bpy.data.objects if o not in before]
    src = next(o for o in new if o.type == "ARMATURE")
    common = [b.name for b in src.data.bones if b.name in arm.data.bones]
    P = np.array([src.matrix_world @ src.data.bones[n].head_local for n in common])
    Q = np.array([arm.matrix_world @ arm.data.bones[n].head_local for n in common])
    s, R, t = similarity(P, Q)
    res = np.linalg.norm(P @ (s * R).T + t - Q, axis=1)
    print(f"outfit: {len(common)} bones, scale {s:.4f}, bind residual mean {res.mean() * 1000:.2f} mm max {res.max() * 1000:.2f} mm")
    fit = Matrix.Identity(4)
    for i in range(3):
        for j in range(3):
            fit[i][j] = s * R[i, j]
        fit[i][3] = t[i]
    pieces = []
    for ob in (o for o in new if o.type == "MESH"):
        ob.data.transform(fit @ ob.matrix_world)
        ob.parent = arm
        ob.matrix_parent_inverse = Matrix.Identity(4)
        ob.matrix_basis = Matrix.Identity(4)
        ob.modifiers.clear()
        for name, colour in OUTFIT_PIECES.items():
            piece = ob.copy()
            piece.data = ob.data.copy()
            piece.name = piece.data.name = name
            bpy.context.scene.collection.objects.link(piece)
            bm = bmesh.new()
            bm.from_mesh(piece.data)
            slots = [m.name if m else "" for m in piece.data.materials]
            word = "Short" if name == "Shorts" else name
            bmesh.ops.delete(bm, geom=[f for f in bm.faces if word not in slots[f.material_index]], context="FACES")
            bmesh.ops.delete(bm, geom=[v for v in bm.verts if not v.link_faces], context="VERTS")
            for f in bm.faces:
                f.material_index = 0
            bm.to_mesh(piece.data)
            bm.free()
            piece.data.materials.clear()
            piece.data.materials.append(outfit_material(name, colour, tex_dir))
            piece.modifiers.new("Armature", "ARMATURE").object = arm
            pieces.append(piece)
    for ob in new:
        bpy.data.objects.remove(ob)
    return pieces


class Under:
    """Whether skin lies beneath one of UE's outfit pieces: a ray out along its normal hits cloth.
    (Nearest-point tests fail on cloth with an inner layer, whose normals face the body.)"""

    def __init__(self, pieces):
        self.trees = [BVHTree.FromPolygons([v.co for v in p.data.vertices], [f.vertices[:] for f in p.data.polygons])
                      for p in pieces]

    def __call__(self, co, normal):
        return any(tree.ray_cast(co + normal * 1e-4, normal, 0.15)[0] is not None for tree in self.trees)


def erode(covered, rings=2):
    for _ in range(rings):
        covered -= {v for v in covered if any(o not in covered for e in v.link_edges for o in e.verts)}
    return covered


def build_clothes(body, head, arm, raw_dir):
    cut = Cut(arm)
    neck = Neck(head)
    outfit = os.path.join(raw_dir, "outfit.fbx")
    pieces = import_outfit(outfit, os.path.join(raw_dir, "outfit_tex"), arm) if os.path.exists(outfit) else []
    under = Under(pieces)
    for name in PROCEDURAL:
        garment(body, neck, name, cut, under)
    # Skin a garment hides entirely goes, two rings in from its edge, so a bent knee or elbow
    # never pushes it through the cloth. The neck (head mesh) stays.
    hide_covered(body, cut, under)


def hide_covered(skin, cut, under):
    bm = bmesh.new()
    bm.from_mesh(skin.data)
    parts = vert_parts(bm, [g.name for g in skin.vertex_groups])
    bm.normal_update()
    covered = erode({v for v in bm.verts if any(cut.keeps(n, parts[v], v.co) for n in PROCEDURAL) or under(v.co, v.normal)})
    bmesh.ops.delete(bm, geom=[f for f in bm.faces if all(v in covered for v in f.verts)], context="FACES")
    bmesh.ops.delete(bm, geom=[v for v in bm.verts if not v.link_faces], context="VERTS")
    bm.to_mesh(skin.data)
    bm.free()


def main():
    argv = sys.argv[sys.argv.index("--") + 1:]
    raw_dir, maps_dir, out = argv
    bpy.ops.wm.read_homefile(use_empty=True)

    bj, barr = load(os.path.join(raw_dir, "body"))
    ej, earr = load(os.path.join(raw_dir, "body_eval"))
    body_names = bj["joints"]
    body_parents = barr("joint_parents").astype(np.int64)
    neutral = earr("neutral_joint_values")[0].reshape(len(body_names), 10).astype(np.float64)
    arm = build_armature(body_names, body_parents, world_matrices(neutral, body_parents))
    body_set = set(body_names)

    hj, harr = load(os.path.join(raw_dir, "head"))
    head_names = hj["joints"]
    head_parents = harr("joint_parents").astype(np.int64)
    joint_map = {}
    for j, name in enumerate(head_names):
        k = j
        while head_names[k] not in body_set and head_parents[k] != k:
            k = head_parents[k]
        joint_map[name] = head_names[k] if head_names[k] in body_set else "head"
    body_map = {n: n for n in body_names}

    mats = {name: material(name, maps_dir) for name in MATERIALS}
    body = build_mesh("Body", barr, "body_lod0_mesh", body_names, body_map, mats["Body"], arm, uv_shift=1.0)

    morphs = np.load(os.path.join(raw_dir, "head_arkit.npz"))
    order = [str(p) for p in morphs["pose_names"]]
    for mesh, mat_name in FACE_MESHES.items():
        ob = build_mesh(mesh.replace("_lod0_mesh", "").capitalize(), harr, mesh, head_names, joint_map, mats[mat_name], arm)
        deltas = to_blender(morphs[mesh]) if mesh in morphs.files else None
        basis = ob.shape_key_add(name="Basis", from_mix=False)
        base = np.array([v.co for v in ob.data.vertices])
        for pose in ARKIT:
            key = ob.shape_key_add(name=pose[0].lower() + pose[1:], from_mix=False)
            if deltas is not None and pose in order:
                d = deltas[order.index(pose)]
                key.data.foreach_set("co", (base + d).astype(np.float32).ravel())

    build_clothes(body, bpy.data.objects["Head"], arm, raw_dir)

    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.export_scene.gltf(
        filepath=out,
        export_format="GLB",
        use_selection=True,
        export_apply=False,
        export_skins=True,
        export_morph=True,
        export_morph_normal=False,
        export_animations=False,
        export_yup=True,
    )
    tris = sum(len(o.data.polygons) for o in bpy.data.objects if o.type == "MESH")
    print(f"wrote {out}: {len(body_names)} bones, {tris} faces")


main()
