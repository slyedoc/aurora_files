#!/usr/bin/env python3
"""A landscape stamp for aurora_landscape's HeightmapLayer (blend Add): a valley dropped
below a cliff line, far peaks, and optional pads lowered under building footprints.

Stamp-local metres, origin at the centre, +z = image rows down. Writes a 16-bit grayscale
PNG and prints the HeightmapLayer fields to use (size, height_min, height_max).

    python3 scripts/valley_stamp.py OUT.png --size 280 --cliff -128 --depth 38 \
        --pad 0,-135,6.95,4.45,3,1.2
"""
import argparse

import numpy as np
from PIL import Image


def smoothstep(a, b, x):
    t = np.clip((x - a) / (b - a), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def value_noise(n, cells, rng):
    grid = rng.random((cells + 2, cells + 2))
    t = np.linspace(0.0, cells, n)
    i = np.floor(t).astype(int)
    f = t - i
    f = f * f * (3.0 - 2.0 * f)
    a = grid[np.ix_(i, i)]
    b = grid[np.ix_(i, i + 1)]
    c = grid[np.ix_(i + 1, i)]
    d = grid[np.ix_(i + 1, i + 1)]
    fx = f[None, :]
    fz = f[:, None]
    return (a * (1 - fx) + b * fx) * (1 - fz) + (c * (1 - fx) + d * fx) * fz


def fbm(n, base, octaves, rng):
    out = np.zeros((n, n))
    amp, total = 1.0, 0.0
    for o in range(octaves):
        out += amp * value_noise(n, base * 2**o, rng)
        total += amp
        amp *= 0.5
    return out / total


def rounded_rect_sd(x, z, cx, cz, hx, hz, r):
    qx = np.abs(x - cx) - (hx - r)
    qz = np.abs(z - cz) - (hz - r)
    outside = np.hypot(np.maximum(qx, 0.0), np.maximum(qz, 0.0))
    inside = np.minimum(np.maximum(qx, qz), 0.0)
    return outside + inside - r


def main():
    p = argparse.ArgumentParser()
    p.add_argument("out")
    p.add_argument("--size", type=float, default=280.0)
    p.add_argument("--texel", type=float, default=0.5)
    p.add_argument("--cliff", type=float, default=-128.0, help="z of the cliff's top edge")
    p.add_argument("--run", type=float, default=7.0, help="metres the cliff takes to drop")
    p.add_argument("--depth", type=float, default=38.0)
    p.add_argument("--width", type=float, default=80.0, help="half width at full depth")
    p.add_argument("--fade", type=float, default=45.0, help="side slope back up to 0")
    p.add_argument("--peaks", type=float, default=90.0)
    p.add_argument("--wobble", type=float, default=30.0, help="metres the valley sides wander")
    p.add_argument("--seed", type=int, default=7)
    p.add_argument("--pad", action="append", default=[],
                   help="cx,cz,half_x,half_z,radius,depth[,apron]: a footprint lowered by depth, "
                        "the ground within apron metres of it 6 cm under the slab, rising back")
    a = p.parse_args()

    n = int(round(a.size / a.texel)) + 1
    rng = np.random.default_rng(a.seed)
    t = np.linspace(-a.size / 2, a.size / 2, n)
    x = t[None, :].repeat(n, 0)
    z = t[:, None].repeat(n, 1)
    edge = a.size / 2

    wobble = (fbm(n, 6, 4, rng) - 0.5) * a.wobble
    s = z - a.cliff + (fbm(n, 10, 3, rng) - 0.5) * 3.0
    drop = smoothstep(0.0, a.run, s) ** 0.8
    sides = 1.0 - smoothstep(a.width, a.width + a.fade, np.abs(x) + wobble)
    floor = (fbm(n, 5, 5, rng) - 0.5) * 8.0 * smoothstep(a.run, a.run * 4, s)
    valley = -(a.depth + floor) * drop * sides

    ridge = 1.0 - np.abs(fbm(n, 4, 5, rng) * 2.0 - 1.0)
    rise = smoothstep(edge * 0.5, edge * 0.85, z) * (1.0 - smoothstep(edge * 0.88, edge, z))
    taper = 1.0 - smoothstep(edge * 0.8, edge, np.abs(x))
    peaks = a.peaks * ridge**1.5 * rise * taper

    h = valley * (1.0 - smoothstep(edge * 0.3, edge * 0.7, z)) + peaks
    h += (-a.depth * 0.3) * smoothstep(edge * 0.3, edge * 0.7, z) * sides * (1 - rise)

    for spec in a.pad:
        v = [float(v) for v in spec.split(",")]
        cx, cz, hx, hz, r, d = v[:6]
        apron = v[6] if len(v) > 6 else 1.5
        sd = rounded_rect_sd(x, z, cx, cz, hx, hz, r)
        near = sd < apron
        h = np.where(near, np.minimum(h, -0.06 * (1.0 - smoothstep(apron * 0.4, apron, sd))), h)
        h = np.where(sd < 0.0, np.minimum(h, -d), h)

    for side in (h[0, :], h[-1, :], h[:, 0], h[:, -1]):
        side[:] = 0.0

    lo, hi = float(h.min()), float(h.max())
    img = np.round((h - lo) / (hi - lo) * 65535.0).astype(np.uint16)
    Image.fromarray(img).save(a.out)
    print(f"size {a.size} x {a.size}, {n} x {n} samples; height_min {lo:.4f}, height_max {hi:.4f}")


if __name__ == "__main__":
    main()
