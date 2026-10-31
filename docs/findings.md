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

## Measuring the real walk, not a model

Horizon ships its own benchmark of the real `Resident::visible`: `resident::tests::column_walk_speed`,
ignored by default, which builds a render-distance-32 world of 80,802 resident sections, runs the
walk 200 times, keeps the best, and asserts it found exactly the sections a per-section reference
walk found. `tools/measure-walk.sh` runs it pristine and patched.

At commit `ba6344a`, **debug profile** (opt-level 0), 80,802 resident and 8,746 visible sections:

| | column walk | checksum |
| --- | ---: | ---: |
| pristine | 13.31 ms | −15,940,400 |
| patch 0002 | **6.33 ms** | −15,940,400 |

A 52% reduction, with the identical checksum — which is the benchmark's own proof that the walk
still finds the same sections in the same order.

**This number must not be quoted as the release gain.** A debug build does not inline or hoist,
so it flatters exactly this kind of change: work the patch removes by hand is work the optimizer
may already be removing for free in an optimized build. If LLVM's loop-invariant code motion
already lifts the column's x and z out of the per-section loop, the release gain could be far
smaller or nothing. That is the measurement still outstanding, and `measure-walk.sh` defaults to
the `fast` profile for precisely that reason.

The distinction is the same one that killed the sort patch: a debug or model measurement can tell
you a change is *real*, and only an optimized measurement can tell you it *matters*.

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
