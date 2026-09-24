# scene_editor — the rung ladder

Build a world out of baked `.bsn` props, on aurora. The end state is the one worth naming: put
the headset on, look down at a shelf of real props, and place them by reaching for them.

Everything here reads the **kit manifest** (`<name>.kit.ron`) that `prop_import --per-group`
writes. That file exists because a bevy `AssetServer` cannot enumerate — there is no "list the
`.bsn` files" call, and a shipped build has no directory to walk. Bake time is the one moment the
contents of a kit are known for free, so that is where the list gets written down.

    cargo run --release -p scene_editor            # every kit under $BEVY_ASSET_ROOT
    scene_editor --kit fantasy_props               # one kit

## R0 — the palette listing (done)

Discover every `*.kit.ron` under the asset root, load it, list what it holds: one selectable row
per prop, its name, and the longest side of the AABB it was baked at. A search box filters.

* Kit discovery scans the filesystem, which is a **tool's** privilege — a tool runs against a
  working tree. Below that point everything is data from the manifest.
* The loader claims the compound extension `kit.ron`, not `ron`. Bevy resolves a loader by trying
  the whole extension chain first and only then the secondary ones, so this takes precedence over
  any plain `.ron` loader in the app without either having to know about the other.
* Rows show `short_name()` — leading SHOUTED segments stripped (`COMP_PROP_cart_city_02` ->
  `cart_city_02`). Kits name things `LIB_CATEGORY_what_it_is`, and in a list where every row
  carries the same shout those segments are the only part guaranteed to be useless. In a narrow
  pane they are also the part that survives clipping while the distinguishing tail is cut.
* The UI is feathers widgets: `FeathersListView` (scroll area, scrollbar, mutually-exclusive
  selection, keyboard navigation) and `FeathersTextInput` (focus, cursor, selection). **Do not
  hand-roll these.** A hand-built filter that reads raw `KeyboardInput` eats the free camera's
  WASD, because nothing tells it whether it has focus; that is what the widget is for.
* Rows are built at runtime: `FeathersListViewProps::rows` is a `Box<dyn SceneList>` and
  `Vec<S: Scene>` implements `SceneList`, so a `bsn!` row function maps over the manifest.

Known: ~50 fps with 297 rows live. Every row is a `ListItem` with hover, theming and an
accessibility node, which is not free. Virtualize if it starts to matter.

## R1 — the shelf

Replace the text rows with **live prop instances** — the real `.bsn`, scaled into a uniform cell
by the manifest's extent (cap that scale at 1, or a teaspoon gets blown up to the size of a
forge). This is cheap here in a way it is not elsewhere: aurora does ~1M shared-BLAS
instances at 240+ fps, so 297 real props cost nothing and no thumbnail bake, render-to-texture or
icon pipeline is needed. The extent in the manifest is what makes the layout possible without
instantiating everything first and measuring it.

## R2 — place

Selection already round-trips (`Selection` holds the chosen prop). Spawn it into the world on a
ground pick, with snapping. Props bake with `min.y ~ 0`, so placement needs no lift.

## R3 — save

Write the placed world back out as a `.bsn`. **This is the rung that makes it an editor rather
than a viewer**, and it is the one piece the stack does not have: the only `.bsn` emitter here is
`aurora_bsn::bsn`, string templates that know about meshes and materials and nothing else.

The shape to copy is jackdaw's `jackdaw_bsn/src/document/from_reflect.rs` — walk `ReflectRef`
over a component and build a patch. Note the inversion between the two codebases: jackdaw drives
**serialization** from reflection and hand-writes its inspector UI; this stack drives the
**inspector UI** from reflection (`bevy_feathers_inspector::recurse`) and hand-writes its `.bsn`.
Each has what the other lacks.

Not needed here: jackdaw's `AstNodeRef` / `ecs_to_ast` machinery, which exists to edit `.bsn`
text in place and preserve comments. A tool that re-emits the scene wholesale does not need it.
Its real payoff is **identity** for per-node change detection, and the cheap version of that —
given the AST is already a `World` — is one component stamped on each spawned entity pointing
back at its AST entity.

## R4 — XR

The point of the exercise. Shelf in reach, grab to place. Needs the enhanced_input XR bindings,
unexamined so far. A ray-pick from the shelf is the fallback if grabbing proves fiddly.
