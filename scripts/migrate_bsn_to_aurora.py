#!/usr/bin/env python3
"""Rewrite `.bsn` files baked for the old engine into aurora's vocabulary, in place.

Old (bevy_aurora_old, f64 transforms)          -> aurora (crate bevy_aurora, f32)
  bevy_aurora::bindings::types::RaytracingMesh3d   bevy_mesh::components::Mesh3d
  bevy_aurora::material::AuroraMaterial3d(         bevy_aurora::bsn::RaytracingMaterial3d(
      bevy_aurora::material::StandardAuroraMaterial   bevy_pbr::pbr_material::StandardMaterial
  bevy_aurora::material::alpha::AlphaMode          bevy_material::alpha::AlphaMode
  glam::DVec3 / glam::DQuat                        glam::Vec3 / glam::Quat
  bevy_animation::animclip::AnimatedScene(..)      (dropped: no .animclip on this engine)

Idempotent; files already in the new vocabulary are left untouched.

    python3 scripts/migrate_bsn_to_aurora.py assets
"""
import re
import sys
from pathlib import Path

SWAPS = [
    ("bevy_aurora::bindings::types::RaytracingMesh3d(", "bevy_mesh::components::Mesh3d("),
    (
        "bevy_aurora::material::AuroraMaterial3d(bevy_aurora::material::StandardAuroraMaterial {",
        "bevy_aurora::bsn::RaytracingMaterial3d(bevy_pbr::pbr_material::StandardMaterial {",
    ),
    ("bevy_aurora::material::alpha::AlphaMode::", "bevy_material::alpha::AlphaMode::"),
    ("glam::DVec3", "glam::Vec3"),
    ("glam::DQuat", "glam::Quat"),
]
ANIMCLIP = re.compile(r"^\s*bevy_animation::animclip::AnimatedScene\([^\n]*\)\n", re.M)


def migrate(path: Path) -> bool:
    text = path.read_text()
    out = text
    for old, new in SWAPS:
        out = out.replace(old, new)
    out = ANIMCLIP.sub("", out)
    if out == text:
        return False
    path.write_text(out)
    return True


def main() -> None:
    roots = [Path(p) for p in sys.argv[1:]] or [Path("assets")]
    changed = total = 0
    for root in roots:
        for path in sorted(root.rglob("*.bsn")):
            total += 1
            if migrate(path):
                changed += 1
                print(f"migrated {path}")
    print(f"{changed}/{total} .bsn files rewritten")


if __name__ == "__main__":
    main()
