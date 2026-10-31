# Findings: where Horizon's frame time goes, and what was wrong about it

This is the analysis behind the patches, kept because most of it is a record of hypotheses that
did not survive contact with the code. Every claim cites a file, and the ones that turned out to
be wrong are marked as wrong.

## The measurement that everything rests on

From the project's own published benchmark reports (`benchmarks/perf/*.json`), on a Ryzen 7
9800X3D with an RTX 5080 at 1280x720. `cpu_p50_ms` is CPU frame work with GPU wait subtracted;
`gpu_p50_ms` is GPU timestamps.

| RD | scene | p50 frame | CPU | GPU | CPU share |
| --- | --- | --- | --- | --- | --- |
| 8 | forest | 0.26 ms | 0.144 | 0.075 | ~55% |
| 16 | village | 0.53 ms | 0.379 | 0.104 | ~72% |
| 32 | village | 0.82 ms | 0.665 | 0.193 | ~81% |
| 32 | ocean | 0.73 ms | 0.596 | 0.315 | ~82% |

The frame is CPU-bound at every render distance. Two caveats that limit what this proves:

- The GPU bound at RD 8 is about 13,300 FPS. Two successful CPU optimizations make the GPU the
  limiter, so this premise has a short shelf life.
- The reports carry no per-frame CPU/GPU correlation, so they say nothing about whether the
  *slowest* frames are CPU or GPU. The premise is proven for the frames nobody feels.

## What was wrong

Recorded because each of these was believed at some point and each cost work before it was
refuted.

**"Port Sodium or VulkanMod."** Horizon already implements Sodium's whole playbook natively —
paged vertex arena (`gfx/arena.rs`), indirect multi-draw (`gfx.rs` `draw_sections`), a CPU BFS
occlusion graph on a worker (`gfx/occlusion.rs`), 24-byte packed vertices (`mesher.rs`), and two
things Sodium does not have: GPU-driven two-phase occlusion culling (`gfx/cull.rs`) and emulated
multi-draw on Metal via a compute-built index list (`gfx/flat.rs`). At RD 16 it averages 2424 FPS
against vanilla+Sodium+Lithium's 1746. Porting would be a regression.

**"`facing_runs` splits each section into up to 6 draws, so ~10⁵ records a frame."** Wrong twice.
`Sides::visible` merges runs across empty buckets, so the maximum is 4 (not 6) and only for one
octant, and the realistic mean is nearer 1.75. `gfx/page_runs.rs`'s own comment puts an ungrouped
RD 32 list at about a thousand multi-draws. The estimate was about 25x too high.

**"Cache per-section draw records across frames."** Worthless. The record embeds the visible-list
index, which moves every frame — but even indexing by a stable slot would not pay, because
`regroup` re-copies every draw into `(band, page)` order regardless and `origins` is
camera-relative and rewritten every frame. The copies dominate, not the index.

**"The RD 32 ocean 1% low comes from far-terrain LOD geometry."** The LOD subsystem is not running
in a benchmark at all: `lod_distance` defaults to 0 and the benchmark driver leaves Horizon's own
settings at their defaults. The hypothesis described a scene the benchmark never built.

**"Replace the translucent sort with an unstable sort for a speedup."** Correct but worthless as a
performance change: `SortState::update` early-outs unless the section-relative point of view
changed, so sorts fire on viewpoint change rather than per frame. It is kept only as correctness
hardening.

**"A counting sort for `regroup`."** Correct, and the wrong call: it needs a second buffer, about
430 KiB of retained memory across four instances. Rejected in favour of the tie-break version,
which allocates nothing.

**"Measure at RD 8."** Statistically and perceptually a write-off. Three repeats give a 95%
interval of about ±7.9% of the mean at RD 8, wider than any effect this project can produce, on a
frame rate 29x a 120 Hz panel. RD 32 village is the target: ±0.8% interval, 81% CPU.

## The stall, which is the only perceptible defect

At RD 32 over an ocean the median frame is 0.731 ms but p999 is 12.478 ms and the maximum is
18.863 ms, with 1% lows at 154 FPS against vanilla's 236. A 15 ms frame is 25x the entire median
frame's CPU work, so no per-frame loop explains it: the tail must be a rare large event or a
**wait**. Four mechanisms are candidates, none measured:

1. **The diagnostic hides waits.** `cpu` is logged as `elapsed - last_wait`, so a frame blocked
   in surface acquire or present prints nothing under `HORIZON_SPIKES`. Unhiding `last_wait` is
   the first diagnostic, before any patch.
2. **Arena page growth mid-frame.** Only `pages.last()` may grow, doubling toward a 268 MB page,
   each growth a synchronous buffer creation plus a copy of the whole previous page. First-fit
   over a `BTreeMap` means holes in older pages are never reusable for large requests, so the
   arena climbs instead of settling. Ocean reports the highest GPU total of the four scenes.
3. **Uploads are unbudgeted by default** (`chunk_updates = 0`, `upload_budget_kib = 0`), so every
   mesh the workers finish is uploaded in the frame that finishes it.
4. **Mass translucency resort.** `Sorter`'s viewpoint cache misses when the camera crosses a
   16-block boundary, at which point every section with translucent geometry resorts in one frame,
   recurring every 16 blocks of travel. Fits a bimodal 0.73 / 12.5 ms distribution better than
   anything steady-state.

Two more costs sit outside the draw path entirely and were absent from the first analysis:
`streaming.update` drains every completed worker result on the main thread each frame, and the
world tick runs inside the frame with a cycle budget that already expects to be exceeded.

## Where the earlier estimates were soft

- `regroup` is called **four** times a frame with shadows enabled, not two or three.
- Its input is **already band-sorted**, because `visible` is sorted by distance and shadow runs are
  pushed in depth order. A stable sort on nearly-sorted input is much cheaper than `n log n`
  suggests, so the payoff of removing its allocation is smaller than the first estimate.
- The draw-list region — the draw-record build, all four `regroup` calls, and the translucency
  resort — **has no span**. Every cost figure about it is an estimate, including the ones in this
  document.
- `spans::describe` filters spans below 0.5 ms, so a `visibility` span worth 0.2–0.35 ms never
  prints. A measurement build has to lower that filter, and there is a unit test pinning it.

## Measuring the real walk: 19.6% off, at opt-level 3

Horizon ships its own benchmark of the real `Resident::visible`:
`resident::tests::column_walk_speed`, ignored by default. It builds a render-distance-32 world of
80,802 resident sections, runs the walk 200 times, keeps the **best** run, and asserts the walk
found exactly the sections a per-section reference walk found. Keeping the best is why a loaded
machine can still give a usable number: a sample that was descheduled is only ever slower.
`tools/measure-walk.sh` runs it pristine and patched.

Measured on an Apple M1 (8 cores) with the `fast` profile (opt-level 3, no LTO), while the machine
was under heavy load from another process — which is exactly the condition the minimum estimator
is for.

| arm | column walk | runs |
| --- | ---: | --- |
| pristine | **542.3 µs** | 539.5, 542.5, 544.75 |
| patch 0002 | **435.9 µs** | 422.5, 434.7, 445.5, 440.9 |

**19.6% off the walk**, and the two arms do not overlap: pristine's fastest run (539.5 µs) is
slower than the patched arm's slowest (445.5 µs). The checksum is identical in every run
(−15,940,400) and the visible count is identical (8,746), so the walk finds the same sections in
the same order.

This also settles the question that made the measurement worth taking: **the optimizer does not
already remove this redundancy.** The concern was that loop-invariant code motion would hoist the
column's x and z out of the per-section loop for free, making the patch pointless. It does not,
and the patch is worth about a fifth of the walk.

The same comparison in a **debug** build read 13.31 ms → 6.33 ms, a 52% win. That number is
inflated and is kept here only as the illustration: a debug build cannot inline or hoist, so it
flatters any change that removes redundancy. Reporting 52% would have been a lie in the flattering
direction, which is the easiest kind to tell by accident. The honest figure is 19.6%.

## The frame-level A/B, and why it cannot settle this on this machine

Patches 0002 and 0003 were built into real `horizon-client` binaries and run against the pristine
build, interleaved A/B/A/B, on Apple M1 (8 cores, integrated 8-core GPU), render distance 32,
1280x720, offscreen, 5 repeats per scene. `tools/ab-pair.sh` interleaves so a steady background
load lands on both arms; `tools/ab-compare.py` reports each arm's mean with its 95% interval and
says whether the intervals separate.

Village, four pairs before the machine got busy:

| arm | avg FPS | 1% low | p50 | cpu_p50 | gpu_p50 |
| --- | --- | --- | --- | --- | --- |
| pristine | 207.2, 206.9, 199.9, 192.7 | 149, 148, 130, 133 | 4.80–5.19 | 2.44–2.69 | 4.32–4.63 |
| patched | 223.2, 200.4, 194.3, 82.4 | 163, 127, 119, 1.7 | 4.45–5.12 | 2.33–2.84 | 3.92–4.58 |

The verdict is **overlapping — no result**, and that is the correct answer rather than a
disappointment. Two things in the data explain it, and neither is a defect in the patch:

1. **This machine is GPU-bound at render distance 32.** `gpu_p50` is about 4.4 ms against
   `cpu_p50` of about 2.5 ms. The CPU has finished its frame long before the GPU has. Removing
   CPU work cannot raise a frame rate that the GPU sets. The published 9800X3D + RTX 5080 figures
   are CPU-bound; this M1 is not the same machine at all.
2. **One patched run was contaminated** (82 FPS, 1% low 1.7 against 119–163 in its siblings),
   which is what makes the patched interval ±87 and produces the "overlapping" verdict. That run
   overlapped with outside work; interleaving does not rescue a sample that collides with a
   compile, it only stops such collisions from landing on one arm preferentially.

So the honest state after both measurements: **patch 0002 makes the visibility walk 19.6% faster,
and that gain is invisible in frame rate on hardware where the GPU is the limit.** To show a
frame-level CPU win needs a machine whose CPU is the bottleneck at the tested scene — which this
one is not, at any render distance tested, and which is exactly what the project's own
9800X3D/5080 reports describe.

Ocean is worse, not better, and settles the question without needing its own A/B: an earlier
pristine run of the same binary shows `gpu_p50` of 7.1–8.7 ms against `cpu_p50` of 3.0–3.7 ms. The
GPU is twice the CPU there. The scene with the worst 1% lows in the published data is the scene
where this machine has the least CPU headroom to give back.

This is worth stating plainly because it is the difference between "the patch works" and "the
patch is worth having on your machine": the first is measured, the second depends on your GPU.

## The frame-level A/B: completed, and negative

Both patches built into real `horizon-client` binaries, pristine against patched, **interleaved
A/B/A/B**, render distance 32, village, offscreen, 5 pairs, Apple M1 (8 cores, integrated GPU).

`tools/ab-pair.sh` interleaves so background load lands on both arms; the analysis below is
**paired** (each patched run against the pristine run taken immediately after it), which is the
right test for a paired design and cancels slow drift.

| metric | pristine | patched | paired diff | t (df=4) | patched lower in |
| --- | ---: | ---: | ---: | ---: | ---: |
| `cpu_p50_ms` | 2.449 | 2.422 | −0.027 ms (−1.1%) | −0.80 | 3 / 5 |
| `gpu_p50_ms` | 3.595 | 3.589 | −0.007 ms | −0.16 | 3 / 5 |
| `avg_fps` | 241.5 | 241.1 | −0.4 FPS | −0.15 | 4 / 5 |
| `low_1pct_fps` | 154.4 | 151.6 | −2.8 FPS | −0.82 | 3 / 5 |
| `p50_ms` | 4.108 | 4.120 | +0.012 ms | +0.24 | 1 / 5 |

Significance at df=4 is |t| > 2.776. **Nothing here is significant, including the 1% lows.** The
result is a negative one and it is reported as such.

Three things make it interpretable rather than just null:

1. **The negative control passes.** `gpu_p50_ms` is unchanged to within 7 µs, and it is unchanged
   *by construction*: the patch touches no GPU work. A change that moved GPU time would mean the
   patch was doing something it does not claim.
2. **The machine is GPU-bound.** GPU 3.6 ms against CPU 2.4 ms at this scene. The CPU finishes its
   frame before the GPU does, so frame rate is set by the GPU and a CPU saving cannot appear in it.
   Shrinking the render target to 480x270 barely moved the GPU (4.4 → 3.6 ms), which says Horizon's
   GPU cost here is **geometry-bound, not fill-bound** — so the usual trick of shrinking the window
   cannot make this machine CPU-bound either.
3. **The effect is below the noise.** The function-level win is 106 µs off the walk. On a 2.4 ms
   CPU frame with run-to-run sd near 60 µs, resolving ~30-50 µs needs on the order of 20 paired
   runs, not 5.

So the honest state after both measurements: **the patch is proven to make the visibility walk
19.6% faster, and that gain does not reach the frame rate on this hardware at this scene.** No
claim is made that it raises FPS or 1% lows, because on the only machine available it does not
measurably do either.

## Why this machine cannot be made CPU-bound

The frame-level A/B is only informative where the CPU sets the frame time. Three separate attempts
to get there, all on Apple M1 (8-core integrated GPU), render distance 32, village:

| attempt | CPU p50 | GPU p50 | frame p50 | outcome |
| --- | ---: | ---: | ---: | --- |
| 1280x720, default graphics | 2.45 ms | 3.60 ms | 4.11 ms | GPU-bound |
| 480x270 (1/7 the pixels) | 2.51 ms | 3.60 ms | 4.08 ms | GPU-bound; shrinking the target barely moves the GPU |
| clouds off (`renderClouds:"false"`) | 3.05 ms | 5.02 ms | 5.63 ms | GPU-bound |

The second row is the informative one: cutting the pixel count by 7x left GPU time unchanged. That
means Horizon's GPU cost on this hardware is **geometry-bound, not fill-bound** — it is paying for
triangles, not for shading pixels. And because triangle count and visible-section count scale
together with render distance, no render distance flips the ratio either: the GPU stays about 1.4x
the CPU at the distances tested.

A fourth attempt scanned render distance itself, on the theory that a smaller world might leave the
CPU as the constraint. Village, 1280x720:

| render distance | frame p50 | CPU p50 | GPU p50 | |
| ---: | ---: | ---: | ---: | --- |
| 6 | 1.10 ms | 0.43 | 0.94 | GPU-bound |
| 10 | 1.38 ms | 0.43 | 1.30 | GPU-bound |
| 16 | 2.19 ms | 0.87 | 2.11 | GPU-bound |
| 24 | 3.38 ms | 1.52 | 3.30 | GPU-bound |
| 32 | 4.87 ms | 2.57 | 4.36 | GPU-bound |

And a fifth attempt used `--test-terrain`, whose simple geometry should strip the GPU of most of
its triangles: RD 16 gave CPU 0.55 against GPU 1.71, RD 32 gave CPU 1.29 against GPU 3.79. Still
GPU-bound.

The GPU is about twice the CPU in every configuration tried. Six independent axes — render distance
6 to 32, two window sizes down to 1/7 the pixels, clouds on and off, two scenes, simple test
terrain, and the span breakdown that puts the GPU at 86% of an ocean frame — and not one of them
makes CPU work the constraint. That is not a limit of the patches; it is a property of the machine.
Anyone can check it in a minute: run any scene, and if `gpu_p50_ms` exceeds `cpu_p50_ms`, a CPU
change cannot move the frame rate there. It is also why the project's own
published numbers come from a Ryzen 7 9800X3D with an RTX 5080, where the CPU is the constraint and
Horizon's 1% lows are the thing worth improving.

A useful consequence for anyone repeating this: the `cpu_p50_ms` and `gpu_p50_ms` fields in the
report tell you which regime you are in before you spend an hour of runs. If GPU p50 exceeds CPU
p50, a CPU change cannot move your frame rate, and no amount of repeats will make it.

## The frame-level A/B, completed: not significant, and a lesson about stopping early

Eight interleaved pairs at RD 32 village, offscreen, Apple M1, paired analysis. One pair was
dropped by a rule fixed in advance (a run whose `cpu_p50` exceeded 3x its own arm's median is not
measuring this patch), leaving seven:

| metric | pristine | patched | paired diff | t (df=6) | patched lower in |
| --- | ---: | ---: | ---: | ---: | ---: |
| `cpu_p50_ms` | 2.574 | 2.535 | −0.039 ms (−1.5%) | −0.65 | 6 / 7 |
| `gpu_p50_ms` | 4.355 | 4.405 | +0.049 ms | +0.69 | 2 / 7 |
| `avg_fps` | 204.9 | 205.0 | +0.2 FPS | +0.06 | 2 / 7 |
| `low_1pct_fps` | 144.5 | 139.7 | −4.8 FPS | −1.22 | 5 / 7 |
| `p50_ms` | 4.865 | 4.855 | −0.010 ms | −0.14 | 5 / 7 |

Significance at df=6 is |t| > 2.447. **Nothing is significant, including the 1% lows.** The
direction on CPU time is consistent — 6 of 7 pairs lower — but small, and the measurement cannot
resolve it.

### The part worth remembering

The **first four pairs of this run** gave `cpu_p50` of 2.570 → 2.445 ms, a 4.9% drop, with
**t = −6.02 and all four pairs favouring the patch**. That is a clean, publishable-looking result,
and it was noise. Three more pairs pulled it to −1.5% and t = −0.65.

Had the run stopped at four pairs — as the previous run did, at five — the repository would now
contain a significant-looking claim that is not true. The only defence is a stopping rule fixed
before the data arrives, not after: collect the planned number, then look. A result that appears
mid-collection and disappears by the end is exactly what a false positive looks like on the way in.

### What this leaves

The patch makes the visibility walk **19.6% faster** — that is measured with separated arms and
reproduces. Its effect on a whole frame at this scene, on this machine, is **below the resolution
of the measurement**. Both statements are true and only the first supports a claim that it makes
Horizon faster for a player.

## Where a slow frame actually goes, measured

`HORIZON_SPIKES=6` makes the game print a span breakdown for every frame slower than 6 ms. One
run at render distance 32, ocean, offscreen, Apple M1, flagged 78 slow frames out of ~1000:

```
frame p50 12.32 ms = cpu 3.16 + gpu 10.63      (GPU is 86% of the frame)
```

Spans inside those 78 slow frames (median and worst, milliseconds):

| span | frames | median | worst |
| --- | ---: | ---: | ---: |
| `renderer` | 78 | 11.4 | 19.1 |
| `encode` | 75 | 2.2 | 6.2 |
| `lt_block` | 5 | 1.8 | 1.9 |
| `clouds` | 11 | 1.7 | 4.3 |
| `streaming` | 8 | 1.6 | 3.6 |
| `update_world` | 42 | 1.2 | 20.1 |
| `upload` | 20 | 0.9 | 1.7 |
| `entity_geometry` | 11 | 0.6 | 19.8 |
| `visibility` | 54 | 0.6 | 0.8 |

Three things this settles, none of them by argument:

1. **`renderer` is 11.4 of a 12.3 ms frame**, and the frame is 86% GPU. The slow frames are not
   the CPU falling behind; they are the GPU taking longer. `submit` — waiting on the GPU queue —
   shows up at 12.2 ms in the worst frame.
2. **`visibility`, the function patch 0002 rewrites, is 0.6 ms** of a 12.3 ms frame: under 5%. A
   19.6% improvement there is about 0.12 ms, or 1% of the frame — which is precisely what the
   frame-level A/B failed to resolve. The function measurement and the frame measurement now agree,
   and the disagreement was never between them: it was my hope that 1% would show through the
   noise.
3. **The CPU costs that are worth attacking on a CPU-bound machine are elsewhere**: `encode` at
   2.2 ms median across 75 of 78 slow frames, `clouds` at 1.7 ms, `upload` at 0.9 ms, and rare
   spikes in `entity_geometry` (19.8 ms) and `update_world` (20.1 ms). None of them is the
   visibility walk, and none of them is what this repository currently patches.

That last point is the honest correction to this project's own premise. `visibility` looked like
the biggest single CPU stage, and on a machine with a fast GPU it may be — but the span data says
that on this one it is a 5% slice, and the stalls that actually hurt are `encode` and a handful of
rare rebuilds.

## Upstream implemented this first

Moving the pin from `ba6344a` (Nightly 2026-10-30) to `68ac6f6` (Nightly 2026-10-31) turned up
something worth knowing before anyone reads the patch as unique work: **`Resident::visible` already
does the hoisting on the newer commit.**

At `68ac6f6` the column loop computes `ox`/`oz` once per column, keeps `min_x`/`min_z`, and squares
the column's share of the distance once — `xz_sq = dx8*dx8 + dz8*dz8` — with the per-section work
reduced to the `y` term. That is the same change `patches/0002` makes. It also splits the `inside`
case into its own loop and adds **per-section bounds** (`section_box(min, self.bounds[slot])`), so
sections are culled against a tight box instead of a fixed 16-cube, which is a better idea than the
one the patch had.

So `patches/0002` is **superseded**: it applies to `ba6344a` and conflicts at `68ac6f6`, and its
benefit is already in the newer tree. The 19.6% measurement on the `ba6344a` walk is still true of
that commit; it just describes work that upstream has since done independently and more
thoroughly.

`patches/0003` still applies at `68ac6f6` and is still the one that removes allocations rather than
duplicating upstream arithmetic.

The lesson generalises: a patch against a fast-moving rewrite has a shelf life measured in days,
and the first thing to check before rebasing is whether the target function still looks the way you
remembered it.

## The measurement that changed a decision

Function-level microbenchmarks were run under load using the minimum over 200 samples, since a
descheduled sample is only ever slower and the minimum approaches the uncontended time. Two of
them were wrong before they were right, which is worth recording:

1. The first version of the `regroup` benchmark reused one `PageRuns` across iterations. After the
   first call the new code took its early-out and skipped the work entirely, reporting an 81%
   improvement that was pure artifact.
2. The first version of the visibility benchmark let the optimizer hoist the column-invariant
   work in *both* arms, so the patch appeared to change nothing. The real walk calls an opaque
   `reachable(key)` per section, which is what blocks that hoisting; once the benchmark modelled
   it, the patch showed a real 12% off that loop.

With both corrected, the answer for the sort change was not neutral but negative: sorting
identical elements with only the comparator changed, the tie-break version is **34–46% slower**.
It was removed. The visibility hoisting showed **12% off the walk** on a faithful model, and the
readback change removes allocations that no microbenchmark can price.

The rule this produced: a correctness proof says a change is *safe*, never that it is *faster*.
Those are different claims and only one of them was being checked.

## What this means for the patches

`0002` (visibility hoisting) is the best-supported change: the redundancy is unambiguous, the
arithmetic is bit-identical by construction, and Horizon's own exact-bits oracle test proves it.
`0003` (readback buffer) targets recurring allocator traffic, which is the 1% low flavour of the
problem. A third patch, for `regroup`'s sort, was measured and rejected — see above. None of the
remaining two has been measured against a frame rate; both are still only proven not to change
what is drawn.

None of them touches the stall list, which is where the perceptible defect actually is. That work
needs instrumentation first, not patches, and it is the honest next step.
