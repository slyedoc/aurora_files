# Physical Lighting Units & Exposure (solari)

Reference video (physical lighting units / exposure): https://www.youtube.com/watch?v=Hr0ZttzVK9M

The recurring "asset values are 0-1.0, I multiply by 1M to see anything" problem (Bistro,
Zero Day, Sponza). This is the units mismatch, and the fix is to anchor everything to
exposure and author in physical units so the magic multiplier disappears.

## Root cause

Two conventions collide:
- Authoring: an emissive factor / 8-bit sRGB texture in [0,1] is a COLOR, not a magnitude.
  It cannot store "how bright" - a dim LED and a neon sign both clamp to 1.0 white. The
  absolute level was never captured (glTF emissiveFactor, C4D luminance color, Bistro).
- Physical: real emitters have luminance (cd/m^2 = nits) or radiance (W/(sr*m^2)), spanning
  ~10 orders of magnitude.

Multiplying Bistro by 1M "to see it" is re-injecting the magnitude the asset threw away -
but blind, as a global fudge, instead of per-source in real units.

## Reference magnitudes

| Source                 | Luminance (nits) | Illuminance (lux) |
|------------------------|-----------------:|------------------:|
| Monitor / phone screen |          200-600 |                 - |
| Overcast sky           |      2,000-8,000 |      1,000-10,000 |
| Clear daylight         |                - |   100,000-120,000 |
| Sun disc               |     ~1.6 x 10^9  |                 - |

Emissive class -> target luminance to author:
| Emissive class      | targetNits (cd/m^2)             |
|---------------------|---------------------------------|
| Computer/TV screen  | 200-500                         |
| Neon / signage      | 1,000-5,000                     |
| Light-bulb surface  | 10,000-100,000 (or a real light)|

## The fix: exposure is the anchor, author in physical units

1. Commit to one unit: nits (cd/m^2) for emissive/luminance, lux for light illuminance.
2. Calibrate the tonemapper once with EV100 (Frostbite/Lagarde):
       maxLuminance = 1.2 * 2^EV100      // scene luminance mapped to white
       exposure     = 1.0 / maxLuminance
       displayColor = tonemap(sceneColor * exposure)
   EV100 becomes the one per-shot knob: ~14-16 daylight exterior, ~5-9 interior.
3. Author emitters as: L_emit = emissiveColor[0,1] * targetNits.
   The [0,1] factor is chromaticity; the nits supply the magnitude. "x2000" now means
   "this is a 2000-nit display" - defensible and reusable, not scene-specific.

## Do it at import (kills the 1M multiplier durably)

glTF already carries physical values - honor them:
- KHR_materials_emissive_strength : scalar multiplier on emissiveFactor = the missing magnitude.
- KHR_lights_punctual : point/spot in candela, directional in lux (already physical).

For assets that lack them (Bistro, classic Sponza): do NOT multiply globally. Apply a
per-material default from the nits table above, keyed on material name/class, store as
emissive_strength on the material. Log when falling back so under-lit assets are visible,
not silent.

## Why "visible" was forcing tiny values

If exposure isn't calibrated, "visible" means "already near display range," which forces
tiny authored values -> hence the 1M hand-scaling. Once exposure is physically anchored
(EV100 already exists in the pipeline, see physical-lighting-units), authored values are
physical and the multiplier is gone.

## Blender/Cycles equivalent (same recipe, different knobs)

- No auto-exposure. EV knob = Color Management -> Exposure (stops), same 2^stops scale.
- Emission Strength is the nits/watts field. Zero Day: 166 emitters all at 1.0 = the Bistro
  problem verbatim (magnitude dropped in C4D->FBX). AgX view transform darkens it further.
- Fix: View Transform -> Standard/Filmic, set Exposure to shot EV, scale emission Strength
  by material class.
