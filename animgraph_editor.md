# animgraph_editor — the animation graph editor on feathers

Port of `bevy_animation_graph`'s editor off egui and onto bevy_feathers, so it rides the same
render-free bevy + aurora stack everything else here does. Upstream's editor is 21k lines, but
that number is misleading:

| | lines | fate |
|---|---:|---|
| `egui_nodes/` (a vendored imnodes port) | 2145 | REPLACED by a feathers canvas |
| egui reflect widgets (`egui_inspector_impls`, `old_reflect_widgets`) | 1761 | REPLACED by `bevy_feathers_inspector` |
| graph layout data, graph mutations, saving, asset tree | 1786 | PORTED nearly as-is |
| FSM editor (1078) | 1078 | PORTED onto the same canvas (R6) |
| event track editor (747) | 747 | PORTED as a timeline (R7) |
| ragdoll editor | 2808 | DEFERRED |
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

A `.anim.ron` opens as a timeline instead: **left-drag a bar** to move an event in time,
**left-click** it to select, wheel to zoom the time axis, middle/right-drag to scroll it.

Canvas controls: **left-drag a node** to move it, **left-click** it to select, **drag pin to pin**
to wire, **right-click an input pin** to cut the link into it, **Delete** to remove the selected
node, **N** for the add-node palette, **middle- or right-drag the background** to pan, **wheel** to
zoom, **Ctrl+S** to save.

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
cannot click.

**R2b — live values (DONE).** Every output pin on the canvas shows what it is CURRENTLY
producing, which is what turns the canvas from a diagram into a debugger: drag `speed` and watch
`fac_walk`, `fac_jog` and `rate` move on the boxes that compute them. Nothing is recomputed
beside the graph — `player.get_context_arena()` reaches the evaluator's own `node_caches`, keyed
by `(StateKey, PinId)`, so a readout is literally what the pose was built from. The cache is
cleared per frame, so a pin that did not take part in THIS frame's evaluation reads empty rather
than stale; that is the honest answer, because a node behind a zero-weight blend genuinely did
not run. This supersedes the `get_outputs` readout originally planned here: a locomotion graph's
only graph-level output is a pose, which has no useful short form, while the INTERESTING numbers
were always the intermediate ones.

Worth stating why this matters beyond convenience. zero's locomotion graph shipped with guessed
`walk_speed` / `jog_speed`, which slid the feet 23% at its walk speed; the arithmetic that proves
it is exactly what these pins display. A wrong blend is now visible rather than deduced.

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

**R4 — canvas, editing (DONE).** Drag a node to move it, click to select (the box tints and its
parameters list), **drag pin to pin to wire**, **right-click an input pin to cut** the link into
it, **Delete** to remove the selected node, **N** for the add-node palette, **Ctrl+S** to save.

The wiring needs no pending-link bookkeeping: `PointerDragDrop` fires on the pin under the cursor
and names the pin the drag began on, so both ends arrive in one event. Each pin row carries a
`PinSocket` holding its `SourcePin`/`TargetPin` AND its `DataSpec`, so `connects()` can reject a
mismatch without going back to the graph — a `None` spec is a TIME pin, which only ever meets
another time pin. While a wire is in flight every pin it could legally land on lights up; that is
the feedback instead of a rubber band, since bevy_ui cannot draw a line to the cursor without a
per-frame respawn, and showing where a drop WOULD take is more use than showing where the cursor
already is.

The palette's catalogue is the type registry itself — a node type is one carrying
`ReflectNodeLike`, and `ReflectDefault` turns a `TypeId` back into an instance. Same pair
upstream's editor uses, so there is no second node registry to keep in step. 41 types today.

Two graph invariants that are easy to get wrong, and are what the unit tests cover:
`graph.add_edge` alone leaves a displaced edge stranded in the forward `edges` map, so re-wiring
an occupied input has to `remove_edge_by_target` first; and `graph.remove_node` leaves every edge
that touched the node dangling, which is a graph that will not evaluate. Both live in `connect`
and `delete_node` rather than inside the observers, so they are testable.

Two things worth knowing about the save. A round trip drops comments, so the LEADING comment block
is carried across by hand (it is where an authored graph keeps its design record) and a `.bak` is
left the first time; and `graph.nodes` is a `HashMap`, so the node list is sorted by id on the way
out or the file churns on every save. `edges_inverted` is a `HashMap` keyed by `TargetPin`, which
is not `Ord`, so that block still reorders — fixing it means a change in the fork's serializer.

**How this is tested without a pair of hands.** `cargo test -p animgraph_editor` covers the wiring
rule and the two graph invariants. The flags cover the rest: `--open`, `--select <node name>`,
`--palette`, `--save-layout`. And `--self-test` (hidden) adds then deletes a node against the LIVE
graph a second after load, because every interactive path writes `Assets<AnimationGraph>` and so
fires `AssetEvent::Modified` at an `AnimationGraphPlayer` mid-playback — the one thing a unit test
cannot reach and a screenshot cannot click. It survives.

**Node parameters, and the root that made them reachable (DONE).** Selecting a node gives a live
editor over its body: enum variant pickers, checkboxes, sliders, writeback — the inspector's
ordinary machinery.

Getting there needed a new root, because `AnimationGraph::nodes` and `edges` are
`#[reflect(ignore)]` and `DynNodeLike` — the `Box<dyn NodeLike>` each node's body sits in —
carries a HAND-WRITTEN `Reflect` impl reporting a tuple struct with ZERO fields. No `ParsedPath`
from the asset root reaches a node, so `InspectorRoot::Asset` (R0) could not help, and neither
could any amount of work on this side.

The fix is `InspectorRoot::Custom { read, write }` (slyedoc/bevy): a pair of plain `fn`s that walk
the world to the value themselves, after which the path is applied to whatever they yield and
every nested struct, enum and list below it behaves as usual. Plain `fn` rather than a boxed
closure so the root stays `Clone + Eq + Hash` like the other three — anything a resolver needs to
know, it reads from the world, which here is the editor's own `Selected` and `CanvasView`.
`NodeLike: Reflect`, so what comes back is the CONCRETE node struct. The write side must reach the
value through something that marks its owner changed (`Assets::get_mut`), or the UI stays
responsive and every edit is inert — which is what `--self-test` checks by flipping a bool through
the resolvers and reading it back.

This also replaces the reason upstream reaches for `ReflectEditProxy`: no proxy type, no
conversion either way, and it generalises to any value behind an ignored field or a collection key
a path cannot spell.

One widget came with it. A `Handle<A>` field otherwise recurses as the enum it is — a
`Strong` / `Uuid` picker over an `Arc`, noise at best and destructive if clicked — so the
inspector now ships a display widget for it, registered per asset type by the app
(`register_type_data::<Handle<GraphClip>, ReflectInspectorWidget>()`), showing the asset path.

**R5 — curves (DONE).** Links are cubic beziers now, one `UiPolyline` entity each where the
manhattan routing took three rectangles. The primitive lives in aurora (`ui_render.rs`,
`ff56585`) and it needed no shader and no pass: `UiQuad::transform` is an arbitrary `Affine2` and
`UiItem::Node` already carries node-LOCAL `size`, `point` and corner radius, so a segment is one
ROTATED quad through the ordinary node path and the round ends come free from the radius the
fragment shader applies in that local space. Overlapping segments by half a thickness at each end
is what makes the joins continuous rather than notching on the outside of every bend.

A curve leaves its source pin horizontally and arrives at its target the same way, which is the
convention every node editor uses; the control offset grows with the horizontal gap so a short
link stays taut and a long one bows. A BACKWARD link (the target sits left of the source, a
feedback edge) gets a wide offset instead, so it bulges around the boxes rather than doubling back
through them. Segment count scales with length, 8 to 28, so forty links do not become ten thousand
quads.

**R6 — the FSM canvas (DONE).** A state machine opens on the SAME canvas as a graph: states are
boxes, transitions are links, drag to move, drag pin-to-pin to add a transition, Ctrl+S (or
`--save-layout`) to write it back through `StateMachineSerial`.

It shares the canvas rather than copying it, and the join is smaller than expected. Canvas
positions are keyed by the raw `Uuid` that BOTH `NodeId` and `StateId` wrap, so one layout map,
one drag handler and one selection serve both; the split is only in what produces the boxes
(`graph_boxes` / `fsm_boxes`) and what resolves the links between them (`graph_links` /
`fsm_links`). `PinSocket` grew `StateOut` / `StateIn`, and `connects()` returns "an edge or a
transition" instead of always an edge.

Two things made this cheaper than the graph canvas was. `StateMachine::states` and `transitions`
are NOT `#[reflect(ignore)]`, so the R0 asset inspector reaches them with no `InspectorRoot::Custom`
needed; and a transition has no TYPE to agree on, so a state needs exactly one pin a side and
none of the `DataSpec` matching. `add_transition_from_ui` also validates both ends and rebuilds
the low-level FSM — the thing that actually runs — so wiring goes through it rather than
`add_transition_unchecked`.

An FSM has no evaluation order to lay out along (a state machine is a cycle by nature), so the
fallback layout is a plain column rather than the graph's longest-path layering.

`assets/anim/mannequin/locomotion.fsm.ron` in zero is the test fixture — and the skeleton of a
real fix: the blend graph under-covers BELOW `walk_speed` by ~15%, because blending a moving gait
against a stationary idle always does, and a machine with a standing state and a moving state is
the shape that solves it.

**R7 — the event-track timeline (DONE).** A `.anim.ron` opens as a TIMELINE rather than a node
canvas: a ruler across the top, one lane per event track, one bar per event, a scrub cursor.
Click a bar to select it, drag it to move it in time, Ctrl+S (or `--save-layout`) to write the
tracks back through `GraphClipSerial`.

It shares the canvas entity and the pan/zoom, but reinterprets them: `zoom` is pixels-per-second
and `pan.x` is a time offset, which is what makes wheel-zoom and middle-drag do the right thing
on a time axis for free. The ruler picks its tick interval from the first of
`0.05 / 0.1 / 0.25 / 0.5 / 1 / 2 / 5` seconds that is at least 80 px apart, so labels never
collide and never thin out however far you zoom.

A clip has NO player here — `AnimationSource` is `Graph`, `Pose` or `None`, with no clip variant
— so the vertical line is a cursor you place, not a playhead that follows something. For
authoring event times that is the right way round anyway; playing a clip in the preview would
mean wrapping it in a one-node graph, which is a separate feature.

Selecting a bar points the inspector at that ONE event, through `InspectorRoot::Custom` again —
`event_tracks` is a `HashMap<String, EventTrack>` and the item inside is found by id, and a
`ParsedPath` can spell neither. The same escape hatch node bodies needed, for a completely
different reason, which is the argument that the root was worth adding rather than special-casing
nodes. What you get is the event's own enum variant picker (`StringId`, `TransitionToState`, …)
plus `start_time` / `end_time` sliders.

Two details that matter in use: a track is kept SORTED by start time, so both the drag and the
inspector re-sort after an edit; and an instantaneous event has `start_time == end_time`, which
would be a zero-width bar, so bars have a minimum width.

`assets/anim/mannequin/Walk_Loop.anim.ron` in zero carries the first real event tracks — two
footfalls and a both-feet-down contact window, which is the classic reason event tracks exist.

**Deferred past R7:** the ragdoll editor. aurora has 3D gizmos, so the blocker I assumed is not
one; it is simply its own piece of work, and nothing in zero has a ragdoll yet. Each is its own
sub-editor upstream and none blocks authoring a locomotion graph.

## Notes

* The editor previews `.bsn` rigs, which is exactly what the `bsn` viewer already does — if the
  two want to share a preview pane later, that is the seam.
* Graph node `id`s are UUIDs upstream. zero's hand-written `locomotion.animgraph.ron` uses
  readable ones (`...-00000000c1d0`); the editor will rewrite them on save and that is fine,
  nothing indexes by id text.
