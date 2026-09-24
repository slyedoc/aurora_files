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
# open one straight away, and preselect a node
cargo run --release -p animgraph_editor -- -a /mnt/code/p/zero/assets \
  -o anim/mannequin/locomotion.animgraph.ron -s "blend walk jog"
# lay a hand-written graph out and write it back, headless
cargo run --release -p animgraph_editor -- -a /mnt/code/p/zero/assets \
  -o anim/human/locomotion.animgraph.ron --save-layout
```

Canvas controls: **left-drag a node** to move it, **left-click** it to select, **middle- or
right-drag the background** to pan, **wheel** to zoom, **Ctrl+S** to save.

## Rungs

**R0 — asset root (DONE).** `bevy_feathers_inspector` could only address a component or a
resource, so an animation graph's node parameters — which live inside an `AnimationGraph` ASSET
— were unreachable. `InspectorRoot::Asset { asset_id, type_id }` resolves through `ReflectAsset`
(slyedoc/bevy `ba9839d7`), with `BuildAssetInspector` mirroring the resource entry point.
Addressed by `UntypedAssetId` so a binding outlives the handle that opened it. The fork already
calls `register_asset_reflect::<AnimationGraph>()`, so nothing is needed on that side.

**R1 — shell + browser (DONE).** Three panes (feathers `pane`/`subpane`): asset list, centre, inspector.
Walk the assets dir for `*.animgraph.ron` / `*.anim.ron` / `*.skn.ron` / `*.fsm.ron` and list them
in a `listview`; selecting one loads it and points the inspector at it via `BuildAssetInspector`.
The browser is a directory TREE (`Expanded` holds the open paths, a dirty flag respawns rows on
toggle, so a collapsed subtree costs nothing). ASCII `+` / `-` markers, because this shell
inherits whatever font feathers ships and a missing glyph reads as tofu.

**R2 — preview (DONE).** Spawn a rig in the centre pane and play the selected graph on it: the
`examples/bodies` wiring, lifted. One feathers slider per `F32` input, seeded from `default_data` (the spec's map has no public
reader) and writing back through `set_input_data`, so the rig re-poses as you drag. `--open
<asset>` opens one at startup: scriptable, and how this is smoke-tested, since a screenshot run
cannot click. Still to do here: read `get_outputs` back as live readouts.

**R3 — canvas, read only (DONE).** A box per node with its real PIN ROWS — inputs down the left,
outputs down the right, from `AnimationNode::new_spec` — plus the two graph-level rails (`inputs`
is all outputs, `outputs` is all inputs), and a link per edge **manhattan-routed** as three
absolutely-positioned rectangles: ordinary bevy_ui, no line primitive and no new render pass.
Time edges are drawn amber and data edges blue, which makes a graph's timing spine readable.
Middle- or right-drag pans, the wheel zooms about the canvas centre (every px is
`canvas * zoom + pan`, and the font sizes scale with it, because bevy_ui has no node scale).

Three things the data did not do as expected:

* The loader gives EVERY node a position whether the file had one or not, so an unlaid-out graph
  arrives as a pile at the origin rather than as `None`. All-zero is the test, and the fallback is
  a longest-path LAYERING (a node's column is one past its deepest predecessor), which for a
  locomotion graph reads left to right in evaluation order.
* Upstream's node names are emoji-prefixed (`∑ Blend`, `⌚ Speed`, `🔄 Loop`). This shell inherits
  whatever font feathers ships, which has none of them, so every title is stripped to ASCII.
* Panning on ANY drag is wrong even before it fights node dragging: a stray press from the window
  manager as the window takes focus arrives as a left-drag and pans the canvas out from under you.
  Pan is middle/right, node drag is left.

**R4 — canvas, editing (PART DONE).** Done: dragging a node box moves it (left button, and it
stops propagating so the canvas does not pan with it); clicking one selects it, tints its box and
lists that node's parameters; Ctrl+S writes the graph back through `AnimationGraphSerializer`,
folding the canvas layout into `editor_metadata`. `--save-layout` does the same headless, which is
both a batch re-layout for a hand-written graph and how the save path is smoke-tested — a
screenshot run cannot press Ctrl+S. `--select <node name>` does the same for the click path.

Two things worth knowing about that save. A round trip drops comments, so the LEADING comment
block is carried across by hand (it is where an authored graph keeps its design record) and a
`.bak` is left the first time; and `graph.nodes` is a `HashMap`, so the node list is sorted by id
on the way out or the file churns on every save. `edges_inverted` is a `HashMap` keyed by
`TargetPin`, which is not `Ord`, so that block still reorders — fixing it means a change in the
fork's serializer.

Still to do here: drag pin-to-pin to make a link, delete key to remove, a node-type menu to add.
The mutations are already written upstream: `ui/actions/graph.rs`.

**Why node parameters are their own panel and not `bevy_feathers_inspector`.** They cannot be the
inspector's: `AnimationGraph::nodes` and `edges` are `#[reflect(ignore)]`, and `DynNodeLike` — the
`Box<dyn NodeLike>` each node's body sits in — carries a HAND-WRITTEN `Reflect` impl that reports
a tuple struct with ZERO fields. So no `ParsedPath` from the asset root reaches a node's body, and
`InspectorRoot::Asset` (R0) cannot help. What does work: `NodeLike: Reflect`, so `inner_ref()` is a
`&dyn PartialReflect` over the CONCRETE node struct, and walking that gives the read-only list
that is there now. Making it EDITABLE wants one of two things — teach `DynNodeLike` to delegate
its reflection to the inner value, or give the inspector a root that resolves through a closure
rather than a `ParsedPath`. Upstream sidesteps both with `ReflectEditProxy`, which converts a node
to and from a plain reflectable proxy struct; that is probably the right seam here too.

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
