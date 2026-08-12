# bevy_make_human

Customizable humanoid characters in Bevy.

Based on [make huamn](https://static.makehumancommunity.org/index.html) and 
[mpfb2 repo](https://github.com/makehumancommunity/mpfb2.git)

There is another bevy plugin for this, [Humentity](https://github.com/emberlightstudios/Humentity), that you should check out.

> This is very early WIP, expect breaking changes, missing features and bugs.

## TLDR;

> Requires git lfs to clone this repo due to assets.

You will need to copy assets/make_human from this repo to your own project's assets folder.

## Asset Enums

I liked the idea of supporting any assets just by letting users drop them in the right folders and a decade plus of community assets, tuturials and tools that bevy users can utilize.  But using string paths to access assets is error prone and lead to runtime errors instead of compile time.

To that end I created a [build.rs](build.rs) as a test of a way to scans the assets dir and generates enums for the assets it finds.  This way users can still drop in any assets they want and the code will update to reflect that(yes this prevents things like dynamic user generated content in realtime, but thats not a problem I have):

It looks for: ```assets/make_human```, supports BEVY_ASSET_ROOT.

> Results so far: 
> I like the component based construction, but customzing not ideal at all, maybe a one time port of assets would be better. Will most likely complete remove this with ___bsn!___ anyway.

### Adding Assets

1. Download asset packs from [MakeHuman Community Asset Packs](https://static.makehumancommunity.org/assets/assetpacks.html) and unzip them into your `assets/make_human/` folder.
2. Run `./tools/make_meta.sh` to create `.obj.meta` files for any new OBJ files.
3. Build, and the enums will update to reflect the new assets.

### Current Assets 

Assets are from mpfb2 repo asset data and make human community [Asset Packs](https://static.makehumancommunity.org/assets/assetpacks.html):

```
animal01, animal02, animal03, animal04, arms01, bodyparts01, bodyparts02, bodyparts03, bodyparts04, bodyparts05, bodyparts06, cheek01, dress01, dress02, dress03, ears01, equipment01, equipment02, equipment03, eyebrows01, eyelashes01, glasses01, glasses02, gloves01, hair01, hair02, hair03, hands01, hats01, hats02, hats03, hats04, masks01, masks02, nose01, pants01, pants02, pants03, poses01, poses02, shirts01, shirts02, shirts03, shoes01, shoes02, shoes03, skins01, skins02, skins03, skirts01, skirts02, suits01, suits02, suits03, system_clothes_materials01, system_hair_materials01, underwear01, underwear02, underwear03, underwear04, makehuman_system_assets
```

I will most likely remove most these and give instructions to download and zip but wanted to get a large variety to test everything.

## TODO

  - [ ] try bevy_animation_graph once [PR 112](https://github.com/mbrea-c/bevy_animation_graph/pull/112) lands
  - [ ] Scroll Crash on mouse wheel, error: min > max, or either was NaN. min = 0.0, max = -4.0
  - [ ] Animations
    - [ ] Aabb, see [skinned mesh Aabb PR](https://github.com/bevyengine/bevy/pull/21837)
    - [ ] Walk cycles
    - [ ] Facial rigs
  - [ ] support [Delete Groups](https://static.makehumancommunity.org/assets/creatingassets/makeclothes/makeclothes_deletegroups.html) instead of ClothingOffset
  - [ ] Morphs
    - [ ] Allow some to be used as MorphWeights, 
        - limit to 64?
    
## License

All assets in assets/make_human come from make human community and mpfb2 and are licensed under :

* CC0 1.0 Universal [LICENSE-CCO](LICENSE-CCO)
 see [mpfb2](https://github.com/makehumancommunity/mpfb2/blob/master/LICENSE.md):

Rest of the code in this repository is dual-licensed under either:

* MIT License ([LICENSE-MIT](LICENSE-MIT) or [http://opensource.org/licenses/MIT](http://opensource.org/licenses/MIT))
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or [http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0))
Code is licensed under MIT or Apache-2.0 at your option.

> TODO: update animation license info


## WIP: ARKit

All 52 ARKit shapes now exist. 20 need manual sculpting:

  Eyes (14) - need Blender sculpting

  | Left               | Right               |
  |--------------------|---------------------|
  | eye-blink-left     | eye-blink-right     |
  | eye-look-down-left | eye-look-down-right |
  | eye-look-in-left   | eye-look-in-right   |
  | eye-look-out-left  | eye-look-out-right  |
  | eye-look-up-left   | eye-look-up-right   |
  | eye-squint-left    | eye-squint-right    |
  | eye-wide-left      | eye-wide-right      |

  Jaw (3) - simple vertex translations

  | Shape       | Description   |
  |-------------|---------------|
  | jaw-forward | chin moves +Z |
  | jaw-left    | chin moves +X |
  | jaw-right   | chin moves -X |

  Mouth (3)

  | Shape       | Description         |
  |-------------|---------------------|
  | mouth-close | lips press together |
  | mouth-left  | mouth shifts left   |
  | mouth-right | mouth shifts right  |