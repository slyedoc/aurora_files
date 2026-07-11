#!/usr/bin/env python3
# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Build a self-contained HTML report from solari_grader runs.

Scans target/solari_grader/<scene>/runs/*.json (written by `solari_grader`,
one per run) and emits a single publishable HTML file: run/scene pickers, a
KPI row, an accuracy-vs-speed Pareto scatter, FLIP + frame-time bars, a
critcmp-style baseline diff, FLIP error maps (when the run was graded with
--maps), and the full table. Vue is inlined from scripts/vendor/, data is
embedded — the file has no external references, so it can be published as-is.

    uv run scripts/grade_report.py                 # -> target/solari_grader/report.html
    uv run scripts/grade_report.py --embed-maps    # base64 the FLIP error maps (bigger file)
"""

import argparse
import base64
import json
import sys
from pathlib import Path


def load_runs(grader_dir: Path, embed_maps: bool) -> list[dict]:
    runs = []
    for run_file in sorted(grader_dir.glob("*/runs/*.json")):
        try:
            run = json.loads(run_file.read_text())
        except (OSError, json.JSONDecodeError) as e:
            print(f"skipping {run_file}: {e}", file=sys.stderr)
            continue
        run["file"] = run_file.stem
        scene_dir = run_file.parent.parent

        def resolve(name: str | None) -> str | None:
            if not name:
                return None
            path = scene_dir / name
            if not path.is_file():
                return None
            if embed_maps:
                data = base64.b64encode(path.read_bytes()).decode()
                return f"data:image/png;base64,{data}"
            # Relative to the report file, which sits in grader_dir.
            return f"{scene_dir.name}/{name}"

        run["truth_render"] = resolve(run.get("truth_render"))
        for row in run.get("rows", []):
            row["map"] = resolve(row.get("map"))
            row["render"] = resolve(row.get("render"))
        runs.append(run)
    runs.sort(key=lambda r: r.get("timestamp", 0))
    return runs


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--grader-dir", type=Path, default=Path("target/solari_grader"))
    parser.add_argument("--out", type=Path, default=None, help="output HTML (default: <grader-dir>/report.html)")
    parser.add_argument("--embed-maps", action="store_true", help="inline all images (renders + FLIP maps) as base64 — fully portable, but large")
    args = parser.parse_args()

    runs = load_runs(args.grader_dir, args.embed_maps)
    if not runs:
        sys.exit(f"no runs found under {args.grader_dir}/*/runs/ — run solari_grader first")

    vendor = Path(__file__).parent / "vendor" / "vue.global.prod.js"
    if not vendor.is_file():
        sys.exit(f"missing {vendor} — curl it from unpkg (vue.global.prod.js)")

    out = args.out or (args.grader_dir / "report.html")
    html = (
        TEMPLATE
        .replace("__VUE__", vendor.read_text())
        .replace("__DATA__", json.dumps(runs))
    )
    out.write_text(html)
    scenes = sorted({r["scene"] for r in runs})
    print(f"report: {out}  ({len(runs)} runs, scenes: {', '.join(scenes)})")


# Palette: the dataviz reference instance (validated) — categorical slots 1-2,
# sequential blue, status good/critical for deltas, chrome/ink tokens.
TEMPLATE = r"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>solari grade report</title>
<style>
:root {
  --surface-1: #fcfcfb; --page: #f9f9f7;
  --ink-1: #0b0b0b; --ink-2: #52514e; --ink-3: #898781;
  --grid: #e1e0d9; --axis: #c3c2b7; --border: rgba(11,11,11,0.10);
  --series-1: #2a78d6; --series-2: #1baf7a; --seq: #2a78d6;
  --delta-good: #006300; --delta-bad: #d03b3b;
}
@media (prefers-color-scheme: dark) {
  :root {
    --surface-1: #1a1a19; --page: #0d0d0d;
    --ink-1: #ffffff; --ink-2: #c3c2b7; --ink-3: #898781;
    --grid: #2c2c2a; --axis: #383835; --border: rgba(255,255,255,0.10);
    --series-1: #3987e5; --series-2: #199e70; --seq: #3987e5;
    --delta-good: #0ca30c; --delta-bad: #d03b3b;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; background: var(--page); color: var(--ink-1);
  font: 14px/1.45 system-ui, -apple-system, "Segoe UI", sans-serif;
}
.wrap { max-width: 1720px; margin: 0 auto; padding: 24px 24px 60px; }
h1 { font-size: 20px; font-weight: 600; margin: 0 0 4px; }
.sub { color: var(--ink-2); margin: 0 0 20px; }
.controls { display: flex; gap: 12px; flex-wrap: wrap; align-items: end; margin-bottom: 20px; }
.controls label { display: block; font-size: 12px; color: var(--ink-2); margin-bottom: 4px; }
select {
  background: var(--surface-1); color: var(--ink-1); border: 1px solid var(--border);
  border-radius: 6px; padding: 6px 10px; font: inherit; min-width: 160px;
}
.card {
  background: var(--surface-1); border: 1px solid var(--border); border-radius: 10px;
  padding: 16px 18px; margin-bottom: 16px; position: relative;
}
.card h2 { font-size: 14px; font-weight: 600; margin: 0 0 2px; }
.card .note { font-size: 12px; color: var(--ink-3); margin: 0 0 12px; }
.kpis { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 12px; margin-bottom: 16px; }
.tile { background: var(--surface-1); border: 1px solid var(--border); border-radius: 10px; padding: 12px 16px; }
.tile .label { font-size: 12px; color: var(--ink-2); }
.tile .value { font-size: 26px; font-weight: 600; margin: 2px 0; }
.tile .who { font-size: 12px; color: var(--ink-3); }
svg text { font: 11px system-ui, sans-serif; fill: var(--ink-2); }
svg .axis-label { fill: var(--ink-3); }
svg .pt-label { fill: var(--ink-1); font-size: 11px; }
.legend { display: flex; gap: 16px; font-size: 12px; color: var(--ink-2); margin: 4px 0 8px; }
.legend .key { display: inline-flex; align-items: center; gap: 6px; }
.swatch { width: 10px; height: 10px; border-radius: 50%; display: inline-block; }
table { border-collapse: collapse; width: 100%; font-variant-numeric: tabular-nums; }
th { text-align: left; font-size: 12px; color: var(--ink-2); font-weight: 500;
     border-bottom: 1px solid var(--axis); padding: 6px 10px 6px 0; }
th.num, td.num { text-align: right; }
td { padding: 6px 10px 6px 0; border-bottom: 1px solid var(--grid); }
tr:last-child td { border-bottom: none; }
.delta { font-size: 12px; white-space: nowrap; }
.delta.good { color: var(--delta-good); }
.delta.bad { color: var(--delta-bad); }
.delta.flat { color: var(--ink-3); }
.tooltip {
  position: fixed; pointer-events: none; z-index: 10; background: var(--surface-1);
  border: 1px solid var(--border); border-radius: 8px; padding: 8px 12px;
  box-shadow: 0 4px 14px rgba(0,0,0,0.18); font-size: 12px; min-width: 140px;
}
.tooltip .t-title { font-weight: 600; color: var(--ink-1); margin-bottom: 4px; }
.tooltip .t-row { display: flex; justify-content: space-between; gap: 14px; }
.tooltip .t-row .v { font-weight: 600; color: var(--ink-1); }
.tooltip .t-row .k { color: var(--ink-2); }
.maps { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 12px; }
.maps figure { margin: 0; }
.maps img { width: 100%; border-radius: 6px; border: 1px solid var(--border); }
.maps figcaption { font-size: 12px; color: var(--ink-2); margin-top: 4px; }
.bar-hit:hover .bar { filter: brightness(1.12); }
.pt-hit:hover .pt { filter: brightness(1.12); }
footer { color: var(--ink-3); font-size: 12px; margin-top: 24px; }
details.config { margin-bottom: 16px; }
details.config summary { cursor: pointer; font-size: 13px; color: var(--ink-2); }
pre.mono { font: 12px ui-monospace, SFMono-Regular, Menlo, monospace; white-space: pre-wrap;
           color: var(--ink-2); background: var(--surface-1); border: 1px solid var(--border);
           border-radius: 8px; padding: 10px 14px; margin: 8px 0 0; }
.warn { color: var(--delta-bad); font-size: 12px; }
.juxta { position: relative; user-select: none; touch-action: none; cursor: ew-resize;
         border-radius: 6px; overflow: hidden; border: 1px solid var(--border); }
.juxta img { width: 100%; display: block; pointer-events: none; }
.juxta-top { position: absolute; inset: 0; }
.juxta-handle { position: absolute; top: 0; bottom: 0; width: 2px; background: var(--surface-1);
                box-shadow: 0 0 0 1px var(--border); }
.juxta-handle::after { content: "◂ ▸"; position: absolute; top: 50%; left: 50%;
  transform: translate(-50%, -50%); background: var(--surface-1); color: var(--ink-2);
  border: 1px solid var(--border); border-radius: 999px; padding: 3px 8px; font-size: 11px;
  white-space: nowrap; }
.juxta-tag { position: absolute; top: 8px; font-size: 11px; background: var(--surface-1);
  color: var(--ink-1); border: 1px solid var(--border); border-radius: 4px; padding: 2px 8px; }
.thumb { height: 44px; display: block; border-radius: 4px; border: 1px solid var(--border);
         cursor: pointer; }
.thumb:hover { filter: brightness(1.1); }
.thumb.selA { outline: 2px solid var(--series-1); outline-offset: 1px; }
.thumb.selB { outline: 2px solid var(--series-2); outline-offset: 1px; }
</style>
</head>
<body>
<div id="app" class="wrap">
  <h1>solari grade report</h1>
  <p class="sub">accuracy (HDR-FLIP vs converged truth) &times; speed, per estimator recipe</p>

  <div class="controls">
    <div><label>scene</label>
      <select v-model="scene"><option v-for="s in scenes" :key="s" :value="s">{{ s }}</option></select></div>
    <div><label>run</label>
      <select v-model="runIdx"><option v-for="(r, i) in sceneRuns" :value="i">{{ runName(r) }}</option></select></div>
    <div><label>baseline (compare)</label>
      <select v-model="baseIdx"><option :value="-1">none</option>
        <option v-for="(r, i) in sceneRuns" :key="r.file" :value="i">{{ runName(r) }}</option></select></div>
  </div>

  <div class="kpis" v-if="run">
    <div class="tile"><div class="label">best accuracy (exam)</div>
      <div class="value">{{ fmt(best.flip?.flip, 4) }}</div>
      <div class="who">{{ best.flip?.recipe || '—' }} · FLIP</div></div>
    <div class="tile"><div class="label">fastest frame</div>
      <div class="value">{{ fmt(best.ms?.frame_ms, 2) }}<span style="font-size:14px"> ms</span></div>
      <div class="who">{{ best.ms?.recipe || '—' }}</div></div>
    <div class="tile"><div class="label">best efficiency</div>
      <div class="value">{{ fmt(best.eff ? best.eff.flip * best.eff.frame_ms : null, 3) }}</div>
      <div class="who">{{ best.eff?.recipe || '—' }} · FLIP·ms</div></div>
    <div class="tile"><div class="label">run</div>
      <div class="value" style="font-size:18px">{{ run.time_secs }}s · {{ run.truth_spp }} spp truth</div>
      <div class="who">{{ runName(run) }}</div></div>
  </div>

  <details class="config" v-if="run && run.default_config">
    <summary>SolariLighting::default() at run time — the config the <b>default</b> recipe graded</summary>
    <pre class="mono">{{ run.default_config }}</pre>
  </details>

  <div class="card" v-if="run">
    <h2>accuracy vs speed</h2>
    <p class="note">lower-left is better; the two populations answer different questions —
      exam points are {{ run.time_secs }}s accumulations, realtime points are single pre-DLSS-RR frames.</p>
    <div class="legend">
      <span class="key"><span class="swatch" style="background: var(--series-1)"></span>accumulated exam</span>
      <span class="key"><span class="swatch" style="background: var(--series-2)"></span>per-frame realtime</span>
    </div>
    <svg :viewBox="'0 0 ' + W + ' ' + H" style="width:100%">
      <line v-for="t in xTicks" :key="'x'+t" :x1="X(t)" :x2="X(t)" :y1="pad.t" :y2="H - pad.b" stroke="var(--grid)" stroke-width="1"/>
      <line v-for="t in yTicks" :key="'y'+t" :x1="pad.l" :x2="W - pad.r" :y1="Y(t)" :y2="Y(t)" stroke="var(--grid)" stroke-width="1"/>
      <line :x1="pad.l" :x2="W - pad.r" :y1="H - pad.b" :y2="H - pad.b" stroke="var(--axis)" stroke-width="1"/>
      <line :x1="pad.l" :x2="pad.l" :y1="pad.t" :y2="H - pad.b" stroke="var(--axis)" stroke-width="1"/>
      <text v-for="t in xTicks" :key="'xt'+t" :x="X(t)" :y="H - pad.b + 16" text-anchor="middle">{{ t }}</text>
      <text v-for="t in yTicks" :key="'yt'+t" :x="pad.l - 6" :y="Y(t) + 4" text-anchor="end">{{ t }}</text>
      <text class="axis-label" :x="(pad.l + W - pad.r) / 2" :y="H - 4" text-anchor="middle">avg frame ms (log)</text>
      <text class="axis-label" :x="12" :y="(pad.t + H - pad.b) / 2" text-anchor="middle"
            :transform="'rotate(-90 12 ' + (pad.t + H - pad.b) / 2 + ')'">FLIP</text>
      <g v-for="p in points" :key="p.recipe" class="pt-hit"
         @pointermove="tipPoint($event, p)" @pointerleave="tip.show = false">
        <circle :cx="X(p.frame_ms)" :cy="Y(p.flip)" r="14" fill="transparent"/>
        <circle class="pt" :cx="X(p.frame_ms)" :cy="Y(p.flip)" r="5"
                :fill="p.prod ? 'var(--series-2)' : 'var(--series-1)'"
                stroke="var(--surface-1)" stroke-width="2"/>
        <text class="pt-label" :x="X(p.frame_ms) + 9" :y="Y(p.flip) + 4">{{ p.recipe }}</text>
      </g>
    </svg>
  </div>

  <div class="card" v-if="base">
    <h2>compare vs baseline</h2>
    <p class="note">{{ runName(base) }} &rarr; {{ runName(run) }} — green is an improvement (lower), red a regression.</p>
    <p class="warn" v-if="defaultDrift">&#9888; the shipped default changed between these runs — the <b>default</b> row compares different configs:</p>
    <pre class="mono" v-if="defaultDrift">baseline: {{ base.default_config }}
current:  {{ run.default_config }}</pre>
    <table>
      <thead><tr><th>recipe</th><th class="num">FLIP</th><th class="num">&Delta;</th>
        <th class="num">avg ms</th><th class="num">&Delta;</th>
        <th class="num">FLIP&middot;ms</th><th class="num">&Delta;</th></tr></thead>
      <tbody>
        <tr v-for="d in diffs" :key="d.recipe">
          <td>{{ d.recipe }}</td>
          <td class="num">{{ fmt(d.flipB, 4) }} &rarr; {{ fmt(d.flip, 4) }}</td>
          <td class="num"><span class="delta" :class="cls(d.dFlip)">{{ arrow(d.dFlip) }}</span></td>
          <td class="num">{{ fmt(d.msB, 2) }} &rarr; {{ fmt(d.ms, 2) }}</td>
          <td class="num"><span class="delta" :class="cls(d.dMs)">{{ arrow(d.dMs) }}</span></td>
          <td class="num">{{ fmt(d.effB, 3) }} &rarr; {{ fmt(d.eff, 3) }}</td>
          <td class="num"><span class="delta" :class="cls(d.dEff)">{{ arrow(d.dEff) }}</span></td>
        </tr>
      </tbody>
    </table>
  </div>

  <div class="card" v-if="run">
    <h2>FLIP by recipe</h2>
    <p class="note">equal-time accuracy; sorted best first (realtime rows are per-frame — see the scatter note)</p>
    <svg :viewBox="'0 0 ' + W + ' ' + barH(sortedRows.length)" style="width:100%">
      <g v-for="(r, i) in sortedRows" :key="r.recipe" class="bar-hit"
         @pointermove="tipRow($event, r)" @pointerleave="tip.show = false">
        <rect :x="0" :y="i * 28" :width="W" height="26" fill="transparent"/>
        <text :x="barL - 8" :y="i * 28 + 17" text-anchor="end">{{ r.recipe }}</text>
        <path class="bar" :d="barPath(barL, i * 28 + 4, barW(r.flip, maxFlip), 18)" fill="var(--seq)"/>
        <text :x="barL + barW(r.flip, maxFlip) + 6" :y="i * 28 + 17">{{ fmt(r.flip, 4) }}</text>
      </g>
    </svg>
  </div>

  <div class="card" v-if="run">
    <h2>avg frame time by recipe</h2>
    <p class="note">median steady-state frame time over the run (ms)</p>
    <svg :viewBox="'0 0 ' + W + ' ' + barH(msRows.length)" style="width:100%">
      <g v-for="(r, i) in msRows" :key="r.recipe" class="bar-hit"
         @pointermove="tipRow($event, r)" @pointerleave="tip.show = false">
        <rect :x="0" :y="i * 28" :width="W" height="26" fill="transparent"/>
        <text :x="barL - 8" :y="i * 28 + 17" text-anchor="end">{{ r.recipe }}</text>
        <path class="bar" :d="barPath(barL, i * 28 + 4, barW(r.frame_ms, maxMs), 18)" fill="var(--seq)"/>
        <text :x="barL + barW(r.frame_ms, maxMs) + 6" :y="i * 28 + 17">{{ fmt(r.frame_ms, 2) }}</text>
      </g>
    </svg>
  </div>

  <div class="card" v-if="maps.length">
    <h2>FLIP error maps</h2>
    <p class="note">magma heatmap of perceptual error vs truth — where each estimator fails</p>
    <div class="maps">
      <figure v-for="r in maps" :key="r.recipe">
        <img :src="r.map" :alt="'FLIP error map: ' + r.recipe" loading="lazy">
        <figcaption>{{ r.recipe }} — FLIP {{ fmt(r.flip, 4) }}</figcaption>
      </figure>
    </div>
  </div>

  <div class="card" v-if="run">
    <h2>results</h2>
    <p class="note" v-if="renders.length">click a render or flip map to send it to the compare slider below (alternates left / right)</p>
    <table>
      <thead><tr><th>recipe</th><th class="num">FLIP</th><th class="num">avg ms</th>
        <th class="num">FLIP&middot;ms</th><th class="num">spp</th>
        <th v-if="renders.length">render</th><th v-if="maps.length">flip</th></tr></thead>
      <tbody>
        <tr v-if="run.truth_render">
          <td>truth</td>
          <td class="num">0</td>
          <td class="num">—</td>
          <td class="num">—</td>
          <td class="num">{{ run.truth_spp }}</td>
          <td><img class="thumb" :class="thumbClass(run.truth_render)" :src="run.truth_render"
                   alt="render: truth" loading="lazy" @click="pick(run.truth_render)"></td>
          <td v-if="maps.length"></td>
        </tr>
        <tr v-for="r in sortedRows" :key="r.recipe">
          <td>{{ r.recipe }}</td>
          <td class="num">{{ fmt(r.flip, 4) }}</td>
          <td class="num">{{ fmt(r.frame_ms, 2) }}</td>
          <td class="num">{{ fmt(r.flip != null && r.frame_ms != null ? r.flip * r.frame_ms : null, 3) }}</td>
          <td class="num">{{ r.spp ?? '—' }}</td>
          <td v-if="renders.length">
            <img v-if="r.render" class="thumb" :class="thumbClass(r.render)" :src="r.render"
                 :alt="'render: ' + r.recipe" loading="lazy" @click="pick(r.render)">
          </td>
          <td v-if="maps.length">
            <img v-if="r.map" class="thumb" :class="thumbClass(r.map)" :src="r.map"
                 :alt="'FLIP map: ' + r.recipe" loading="lazy" @click="pick(r.map)">
          </td>
        </tr>
      </tbody>
    </table>
  </div>

  <div class="card" v-if="allImages.length >= 2">
    <h2>image compare</h2>
    <p class="note">click renders in the table above, or pick any two images (renders / FLIP maps, any run) — then drag the divider</p>
    <div class="controls">
      <div><label>left</label>
        <select v-model="juxtaA"><option v-for="(m, i) in allImages" :key="m.key" :value="i">{{ m.label }}</option></select></div>
      <div><label>right</label>
        <select v-model="juxtaB"><option v-for="(m, i) in allImages" :key="m.key" :value="i">{{ m.label }}</option></select></div>
    </div>
    <div class="juxta" @pointerdown="juxtaDrag" @pointermove="juxtaMove" @pointerup="juxtaUp">
      <img :src="allImages[juxtaB].src" :alt="allImages[juxtaB].label">
      <div class="juxta-top" :style="{ clipPath: 'inset(0 ' + (100 - juxtaPos) + '% 0 0)' }">
        <img :src="allImages[juxtaA].src" :alt="allImages[juxtaA].label">
      </div>
      <div class="juxta-handle" :style="{ left: juxtaPos + '%' }"></div>
      <span class="juxta-tag" style="left: 8px">{{ allImages[juxtaA].label }}</span>
      <span class="juxta-tag" style="right: 8px">{{ allImages[juxtaB].label }}</span>
    </div>
  </div>

  <div class="tooltip" v-show="tip.show" :style="{ left: tip.x + 'px', top: tip.y + 'px' }">
    <div class="t-title">{{ tip.title }}</div>
    <div class="t-row" v-for="row in tip.rows" :key="row[0]"><span class="k">{{ row[0] }}</span><span class="v">{{ row[1] }}</span></div>
  </div>

  <footer>generated by scripts/grade_report.py · solari_grader · NVIDIA HDR-FLIP</footer>
</div>

<script>__VUE__</script>
<script>
const RUNS = __DATA__;
const { createApp } = Vue;
createApp({
  data() {
    const scenes = [...new Set(RUNS.map(r => r.scene))].sort();
    return {
      scenes, scene: scenes[0], runIdx: 0, baseIdx: -1,
      W: 1620, H: 420, pad: { l: 60, r: 150, t: 12, b: 40 }, barL: 170,
      tip: { show: false, x: 0, y: 0, title: '', rows: [] },
      juxtaA: 0, juxtaB: 1, juxtaPos: 50, juxtaDown: false, pickNext: 'A',
    };
  },
  created() { this.runIdx = this.sceneRuns.length - 1; },
  watch: {
    scene() {
      this.runIdx = this.sceneRuns.length - 1; this.baseIdx = -1;
      this.juxtaA = 0; this.juxtaB = 1; this.juxtaPos = 50; this.pickNext = 'A';
    },
  },
  computed: {
    sceneRuns() { return RUNS.filter(r => r.scene === this.scene); },
    run() { return this.sceneRuns[this.runIdx]; },
    base() { return this.baseIdx >= 0 && this.baseIdx !== this.runIdx ? this.sceneRuns[this.baseIdx] : null; },
    rows() { return this.run ? this.run.rows : []; },
    sortedRows() {
      return [...this.rows].sort((a, b) => (a.flip ?? 1e9) - (b.flip ?? 1e9));
    },
    msRows() {
      return this.rows.filter(r => r.frame_ms != null).sort((a, b) => a.frame_ms - b.frame_ms);
    },
    points() {
      return this.rows
        .filter(r => r.flip != null && r.frame_ms != null)
        .map(r => ({ ...r, prod: !r.spp }));
    },
    best() {
      const exam = this.points.filter(p => !p.prod);
      const flip = exam.length ? exam.reduce((a, b) => (a.flip < b.flip ? a : b)) : null;
      const ms = this.points.length ? this.points.reduce((a, b) => (a.frame_ms < b.frame_ms ? a : b)) : null;
      const eff = this.points.length
        ? this.points.reduce((a, b) => (a.flip * a.frame_ms < b.flip * b.frame_ms ? a : b)) : null;
      return { flip, ms, eff };
    },
    xDomain() {
      const v = this.points.map(p => p.frame_ms);
      if (!v.length) return [1, 1000];
      return [Math.min(...v) / 1.5, Math.max(...v) * 1.5];
    },
    xTicks() {
      const [lo, hi] = this.xDomain, ticks = [];
      for (let e = -1; e <= 4; e++) for (const m of [1, 3]) {
        const t = m * 10 ** e;
        if (t >= lo && t <= hi) ticks.push(t);
      }
      return ticks;
    },
    maxFlipPts() {
      return Math.max(0.05, ...this.points.map(p => p.flip)) * 1.15;
    },
    yTicks() {
      const step = this.maxFlipPts > 0.3 ? 0.1 : this.maxFlipPts > 0.12 ? 0.05 : 0.01;
      const ticks = [];
      for (let t = 0; t <= this.maxFlipPts; t += step) ticks.push(+t.toFixed(2));
      return ticks;
    },
    maxFlip() { return Math.max(1e-9, ...this.rows.map(r => r.flip ?? 0)); },
    maxMs() { return Math.max(1e-9, ...this.rows.map(r => r.frame_ms ?? 0)); },
    maps() { return this.sortedRows.filter(r => r.map); },
    renders() { return this.sortedRows.filter(r => r.render); },
    allImages() {
      const out = [], seenTruth = new Set();
      for (const run of this.sceneRuns) {
        const tag = run.label || this.runName(run);
        if (run.truth_render && !seenTruth.has(run.truth_render)) {
          seenTruth.add(run.truth_render);
          out.push({ src: run.truth_render, key: 'truth/' + run.file, label: 'truth · render' });
        }
        for (const r of run.rows) {
          if (r.render) out.push({ src: r.render, key: run.file + '/' + r.recipe + '/r', label: tag + ' · ' + r.recipe + ' · render' });
          if (r.map) out.push({ src: r.map, key: run.file + '/' + r.recipe + '/m', label: tag + ' · ' + r.recipe + ' · flip' });
        }
      }
      return out;
    },
    defaultDrift() {
      return this.base && this.run
        && this.base.default_config && this.run.default_config
        && this.base.default_config !== this.run.default_config;
    },
    diffs() {
      if (!this.base) return [];
      const baseBy = Object.fromEntries(this.base.rows.map(r => [r.recipe, r]));
      return this.sortedRows.flatMap(r => {
        const b = baseBy[r.recipe];
        if (!b || r.flip == null || b.flip == null) return [];
        const eff = r.flip * r.frame_ms, effB = b.flip * b.frame_ms;
        return [{
          recipe: r.recipe,
          flip: r.flip, flipB: b.flip, dFlip: pct(b.flip, r.flip),
          ms: r.frame_ms, msB: b.frame_ms, dMs: pct(b.frame_ms, r.frame_ms),
          eff, effB, dEff: pct(effB, eff),
        }];
      });
    },
  },
  methods: {
    runName(r) {
      const d = new Date(r.timestamp * 1000);
      const when = d.toLocaleDateString() + ' ' + d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      return r.label ? `${r.label} (${when})` : when;
    },
    fmt(v, digits) { return v == null ? '—' : v.toFixed(digits); },
    X(ms) {
      const [lo, hi] = this.xDomain;
      const f = (Math.log10(ms) - Math.log10(lo)) / (Math.log10(hi) - Math.log10(lo));
      return this.pad.l + f * (this.W - this.pad.l - this.pad.r);
    },
    Y(flip) {
      const f = flip / this.maxFlipPts;
      return this.H - this.pad.b - f * (this.H - this.pad.t - this.pad.b);
    },
    barW(v, max) { return v == null ? 0 : (v / max) * (this.W - this.barL - 80); },
    barH(n) { return n * 28 + 4; },
    // Rounded at the data end (right), square at the baseline (left).
    barPath(x, y, w, h) {
      const r = Math.min(4, w);
      return `M${x},${y} h${w - r} a${r},${r} 0 0 1 ${r},${r} v${h - 2 * r} a${r},${r} 0 0 1 -${r},${r} h-${w - r} Z`;
    },
    cls(d) { return d < -0.5 ? 'good' : d > 0.5 ? 'bad' : 'flat'; },
    arrow(d) {
      const a = d < -0.5 ? '▼' : d > 0.5 ? '▲' : '→';
      return `${a} ${Math.abs(d).toFixed(1)}%`;
    },
    pick(src) {
      const i = this.allImages.findIndex(m => m.src === src);
      if (i < 0) return;
      if (this.pickNext === 'A') { this.juxtaA = i; this.pickNext = 'B'; }
      else { this.juxtaB = i; this.pickNext = 'A'; }
    },
    thumbClass(src) {
      return {
        selA: this.allImages[this.juxtaA]?.src === src,
        selB: this.allImages[this.juxtaB]?.src === src,
      };
    },
    juxtaDrag(ev) {
      this.juxtaDown = true;
      ev.currentTarget.setPointerCapture(ev.pointerId);
      this.juxtaMove(ev);
    },
    juxtaMove(ev) {
      if (!this.juxtaDown) return;
      const rect = ev.currentTarget.getBoundingClientRect();
      this.juxtaPos = Math.min(100, Math.max(0, ((ev.clientX - rect.left) / rect.width) * 100));
    },
    juxtaUp() { this.juxtaDown = false; },
    tipAt(ev) { this.tip.x = ev.clientX + 14; this.tip.y = ev.clientY + 14; this.tip.show = true; },
    tipPoint(ev, p) {
      this.tipAt(ev);
      this.tip.title = p.recipe;
      this.tip.rows = [
        ['FLIP', this.fmt(p.flip, 4)],
        ['avg ms', this.fmt(p.frame_ms, 2)],
        ['FLIP·ms', this.fmt(p.flip * p.frame_ms, 3)],
        ['spp', p.spp || 'per-frame'],
      ];
    },
    tipRow(ev, r) {
      this.tipAt(ev);
      this.tip.title = r.recipe;
      this.tip.rows = [
        ['FLIP', this.fmt(r.flip, 4)],
        ['avg ms', this.fmt(r.frame_ms, 2)],
        ['spp', r.spp || 'per-frame'],
      ];
    },
  },
}).mount('#app');
function pct(from, to) { return from ? ((to - from) / from) * 100 : 0; }
</script>
</body>
</html>
"""

if __name__ == "__main__":
    main()
