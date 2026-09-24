# animgraph_editor — the animation graph editor on feathers

Port of `bevy_animation_graph`'s editor off egui and onto bevy_feathers, so it rides the same
render-free bevy + aurora stack everything else here does. Upstream's editor is 21k lines, but
that number is misleading:

| | lines | fate |
|---|---:|---|
| `egui_nodes/` (a vendored imnodes port) | 2145 | REPLACED by a feathers canvas |
| egui reflect widgets (`egui_inspector_impls`, `old_reflect_widgets`) | 1761 | REPLACED by `bevy_feathers_inspector` |
| graph layout data, graph mutations, saving, asset tree | 1786 | PORTED nearly as-is |
| FSM editor, event track editor, ragdoll editor | 4633 | DEFERRED |
| windows, panes, generic widgets | ~10.6k | mostly egui glue; a fraction survives |

The editor is a separate binary and does NOT have to be render-free — but it is anyway, because
feathers is, which is the whole reason this port is cheaper than keeping egui alive on a wgpu
version that cannot co-resolve with ours (see zero's `docs/implementation-notes.md`, Deviations).

## What it opens

Any assets directory, the way the `bsn` viewer does:

```sh
cargo run --release -p animgraph_editor -- -a /mnt/code/p/zero/assets
```

## Rungs

**R0 — asset root (DONE).** `bevy_feathers_inspector` could only address a component or a
resource, so an animation graph's node parameters — which live inside an `AnimationGraph` ASSET
— were unreachable. `InspectorRoot::Asset { asset_id, type_id }` resolves through `ReflectAsset`
(slyedoc/bevy `ba9839d7`), with `BuildAssetInspector` mirroring the resource entry point.
Addressed by `UntypedAssetId` so a binding outlives the handle that opened it. The fork already
calls `register_asset_reflect::<AnimationGraph>()`, so nothing is needed on that side.

**R1 — shell + browser.** Three panes (feathers `pane`/`subpane`): asset list, centre, inspector.
Walk the assets dir for `*.animgraph.ron` / `*.anim.ron` / `*.skn.ron` / `*.fsm.ron` and list them
in a `listview`; selecting one loads it and points the inspector at it via `BuildAssetInspector`.
Port `tree.rs` for the directory walk. No canvas yet — this alone already beats hand-editing RON.

**R2 — preview.** Spawn a rig in the centre pane and play the selected graph on it: the
`examples/bodies` wiring, lifted. Drive the graph's `io_spec.input_data` from generated widgets
(one slider per `F32` input) and read `get_outputs` back as live values. This is the rung that
pays for the whole tool — it is the thing a text editor fundamentally cannot do.

**R3 — canvas, read only.** Port `graph_show.rs`'s LAYOUT half (node boxes, pin rows, link
endpoints) and draw it: nodes are feathers panes at absolute positions, pins are small nodes,
links are **manhattan-routed** — three absolutely-positioned rectangles per link, so this rung
needs no new rendering at all. Pan by dragging the background, zoom by recomputing px positions
(bevy_ui has no node scale). Selecting a node points the inspector at that node's parameters.

**R4 — canvas, editing.** Drag nodes to move them (write back to `editor_metadata.node_positions`),
drag pin-to-pin to make a link, delete key to remove, a node-type menu to add. The mutations are
already written: `ui/actions/graph.rs`. Saving is `AnimationGraphSerializer` + `ui/actions/saving.rs`.
Hit-testing is `ui_picking`, which zero already enables.

**R5 — curves.** Swap manhattan links for beziers. aurora's `UiQuad` carries an arbitrary
`Affine2`, so a curve is N thin ROTATED quads through the existing pipeline — it needs a small
`UiPolyline` extract in `ui_render.rs`, not a new shader or pass.

**Deferred past R5:** the FSM editor, the event-track editor, the ragdoll editor. Each is its own
sub-editor upstream and none blocks authoring a locomotion graph.

## Notes

* The editor previews `.bsn` rigs, which is exactly what the `bsn` viewer already does — if the
  two want to share a preview pane later, that is the seam.
* Graph node `id`s are UUIDs upstream. zero's hand-written `locomotion.animgraph.ron` uses
  readable ones (`...-00000000c1d0`); the editor will rewrite them on save and that is fine,
  nothing indexes by id text.
