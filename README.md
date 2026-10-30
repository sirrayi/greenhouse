# greenhouse

Renderer performance for [Horizon](https://github.com/horizon-rust/horizon), shipped as a
native [horizon-mixins](https://github.com/horizon-rust/horizon-mixins) mod.

Horizon is a from-scratch Rust rewrite of Minecraft: Java Edition 26.3. It is *already* much
faster than Java Minecraft with Sodium, and this mod exists to take the next step, not the
first one. Greenhouse changes the game's own Rust source at build time through declared mixin
injections; the launcher builds it on the player's copy. There is no fork and no Java.

**Status: work in progress. One change is proven correct, none is yet proven faster.**
See [Status](#status) for exactly what is and is not verified.

---

## Read this first: what Horizon already is

The obvious move for "more FPS" is to port Sodium or VulkanMod. That would be a regression,
and the project's own benchmarks say so. Sodium's entire playbook is already implemented
natively in `horizon-client-render`:

| Sodium's technique | Horizon's equivalent | Source |
| --- | --- | --- |
| Mega-buffer / region allocator | Paged vertex arena, first-fit free list, 256 MiB pages | `gfx/arena.rs` |
| `glMultiDrawElementsBaseVertex` | `multi_draw_indexed_indirect` over page-runs | `gfx.rs` `draw_sections` |
| Compact vertices | 24-byte packed vertex (pos, uv, layer, light, rgba8) | `mesher.rs` `TerrainVertex` |
| CPU BFS occlusion graph | Same graph, built on a worker; frames never wait for it | `gfx/occlusion.rs` |
| — | **GPU-driven two-phase occlusion culling** (depth-test trick) | `gfx/cull.rs`, `cull.wgsl` |
| — | **Emulated multi-draw on Metal** via a compute-built index list | `gfx/flat.rs` |
| — | Far-terrain LOD tiers | `lod.rs` |

Measured by the project on a Ryzen 7 9800X3D + RTX 5080, 1280x720
(`benchmarks/perf/windows-9800x3d-5080-2026-10-29.md`): at render distance 16, Horizon
averages **2424 FPS against vanilla+Sodium+Lithium's 1746**, with 1% lows of **709 vs 461**.

So the headroom is not where the mod ecosystem usually looks.

## Where the time actually goes

From the project's own benchmark report JSON (`benchmarks/perf/*.json`), `cpu_p50_ms` is the
CPU-side frame work with GPU wait subtracted, and `gpu_p50_ms` is GPU timestamps:

| RD | scene | p50 frame | CPU | GPU | CPU share of frame |
| --- | --- | --- | --- | --- | --- |
| 8 | forest | 0.26 ms | 0.144 | 0.075 | ~55% |
| 16 | village | 0.53 ms | 0.379 | 0.104 | ~72% |
| 32 | village | 0.82 ms | 0.665 | 0.193 | ~81% |
| 32 | ocean | 0.73 ms | 0.596 | 0.315 | ~82% |

**The frame is CPU-bound at every render distance.** The GPU is idle roughly half of every
frame at RD 8. That is why this mod attacks per-frame CPU work — draw-list construction,
sorting, allocation — and not shaders, culling hardware or vertex format.

It also explains the one place Horizon currently loses to vanilla: at RD 32 over an ocean its
average FPS matches vanilla but its 1% low is 155 FPS against 236 (0.1%: 68 against 99), on an
idle machine. A median frame of 0.73 ms with 1% lows near 6.5 ms is not a throughput problem,
it is a **stall** problem, and it is a separate line of work from the CPU-throughput changes
here.

## The changes

Everything is individually switchable: `HORIZON_GREENHOUSE=off` disables the mod,
`HORIZON_GREENHOUSE=-sort_unstable` disables one change and keeps the rest.

| ID | Change | Location | Output identical? | Speedup |
| --- | --- | --- | --- | --- |
| `sort_unstable` | Translucent quad sort: unstable sort with an explicit index tie-break instead of a stable sort that allocates a scratch buffer on every call | `gfx/translucency.rs` `Sorter::sort` | **Proven** (below) | Unmeasured |
| `regroup_counting` | Draw regrouping: counting sort over the bounded `(band, page)` key instead of `sort_by_key` + a double scratch copy | `gfx/page_runs.rs` `PageRuns::regroup` | Designed | Unmeasured |
| `band_no_sqrt` | Distance band from a threshold compare instead of an `f64` square root per section per frame | `gfx/page_runs.rs` `PageRuns::push` | Designed | Unmeasured |
| `cull_scratch` | Occlusion readback: reuse a scratch buffer instead of allocating a `Vec` per readback | `gfx/cull.rs` `begin` | Designed | Unmeasured |

### `sort_unstable`, and why it is exactly equivalent

Horizon sorts each translucent section's quads farthest-first, and preserves mesh order for
quads at equal distance by using a **stable** sort. Rust's stable `sort_by` allocates an
`n / 2` scratch buffer on every call — and this runs once per resorted section, per frame.

Adding the quad index as an explicit tie-break makes the comparator a total order over
distinct elements, so the unstable sort has exactly one admissible result and it is the stable
one. Same index buffer, no allocation.

`mixins/greenhouse/runtime.rs` carries a reference copy of Horizon's implementation and its
replacement, and `cargo test` proves they agree on:

- the three cases Horizon's own translucency unit tests pin;
- up to 257 identical quads, where ties alone decide the order;
- 400 randomized sections with quarter-block coordinates, so ties are frequent;
- `+0.0`/`-0.0`, `NaN` and infinities, where `total_cmp`'s total order matters.

```
running 5 tests
test tests::identical_on_the_existing_unit_cases ... ok
test tests::identical_under_massive_distance_ties ... ok
test tests::identical_on_random_sections_with_frequent_ties ... ok
test tests::identical_on_degenerate_inputs ... ok
test tests::every_switch_can_be_turned_off_by_name ... ok
```

Correctness is not the open question for this change. Whether it is *worth* anything is: at
RD 8 a frame is 0.26 ms and the change may be lost in noise.

## Status

| Item | State |
| --- | --- |
| Horizon beats vanilla + Sodium + Lithium | **Confirmed** from the project's published benchmark |
| The frame is CPU-bound | **Confirmed** from the project's benchmark report JSON |
| `sort_unstable` produces identical output | **Proven** — 5 equivalence tests, `cargo test` |
| `sort_unstable` is faster | **Unmeasured** — see the caveat below |
| The other three changes | **Designed, not written** |
| This bundle plans/applies/compiles under horizon-mixins | **Unverified** |
| End-to-end FPS A/B against pristine Horizon | **Not started** |

**Nothing here has been benchmarked yet, on purpose.** Every measurement so far was taken with
the machine at load 27 on 8 cores, and under that load a microbenchmark of the sort swung
between −55% and +148% on identical inputs run to run. Numbers like that are worse than no
numbers. The switch to unstable sorting is a safe change with a small expected win; it is not
yet a demonstrated one.

## Verifying it

The mod is pinned to the tagged Nightly `nightly-source/2026-10-30-ba6344a`
(`ba6344a59cb9e755b730dcf02285fc0d33b97d3c`). The three functions this mod targets are
byte-identical between that Nightly and later `main`, so the analysis carries across revisions
even though `gfx.rs` elsewhere has moved.

```sh
# 1. The equivalence proof, with no Horizon, GPU or assets involved.
cargo test --release

# 2. Is the change even worth having? Run only on an idle machine, several times.
cargo test --release -- --ignored --nocapture

# 3. Does it plan, apply and compile against the pinned Horizon?
#    See tools/verify.sh, which drives horizon-mixins plan/apply/check
#    against a pristine export of the pinned revision.
tools/verify.sh

# 4. Does it make the game faster, and not slower anywhere?
#    Pristine and modded builds, same machine, same world, same camera path:
#    horizon-client --benchmark standard --offscreen, then compare the reports.
```

Step 3 is the immediate blocker: horizon-mixins' own tooling is validated on Linux, and this
work is currently happening on macOS, where the staging tools use `fcntl` locks and Python
3.12+. Until it runs, the manifest is a design, not a working mod.

## Rules this mod follows

- **Identical output over speed.** A change that alters what is on screen is not a performance
  change, it is a parity bug. `sort_unstable` is the model: provable equivalence, then measure.
- **Never trade memory for frame time on weak devices.** No change here may increase resident
  memory or VRAM. Every one of them removes per-frame allocation; that helps an 8 GB machine.
- **No silent regressions.** Each change is separately switchable, so a device that regresses
  can be narrowed to one switch instead of losing the whole mod.
- **Optimize what is actually binding.** CPU work here, not GPU work, because that is what the
  measurements say.

## Attribution

Horizon is MIT, by the horizon-rust contributors. horizon-mixins is MIT, by levzzz5154.
Greenhouse is MIT. It contains no Horizon source beyond the reference implementation copied
into `runtime.rs` and the hook bodies it declares, both marked as such.

---

## What the stall audit found (and what it refuted)

The RD 32 ocean 1% low was first assumed to come from far-terrain LOD geometry. **That was
wrong.** `lod_distance` defaults to 0 (`graphics.rs`) and the benchmark driver leaves Horizon's
own settings at their defaults (`docs/benchmark.md`), so the LOD subsystem is never constructed
during a benchmark run. Any hypothesis starting from "lots of far LOD geometry" is describing a
scene the benchmark never built.

What the numbers do say: the ocean flight's median frame is 0.73 ms while its slowest frames
reach 18.9 ms, and there are only ~84 frames above 4.17 ms in ~14,300. A 15 ms frame is 25× the
whole median frame's CPU work, so the tail cannot be "a bit more of the usual per-frame loop".
It must be a rare large event on the frame thread, or a **wait**.

Three properties of the client make those the live suspects:

1. **The diagnostic hides the wait.** `draw_frame` measures `last_wait` (surface acquire +
   present) and the logged `cpu` subtracts it, while the benchmark's frame interval records it
   in full. A frame blocked 15 ms in acquire prints nothing under `HORIZON_SPIKES`.
   *Unhiding `last_wait` is the first diagnostic, before any optimization.*
2. **The terrain arena grows monotonically mid-frame.** Only `pages.last()` may grow, doubling
   toward a 268 MB page, and each growth is a synchronous buffer creation plus a copy of the
   entire previous page recorded into the current frame's encoder. Combined with first-fit over
   a `BTreeMap`, holes in older pages are never reusable for large requests, so the arena keeps
   climbing instead of reaching steady state. Ocean reports the highest GPU total of the four
   scenes (2085 MB).
3. **Uploads are unbudgeted by default.** `chunk_updates = 0` and `upload_budget_kib = 0` mean
   every mesh the workers finish is uploaded in the frame it finishes.

A separate, scene-independent suspect worth a cheap A/B: fancy clouds rebuild in one 4–8 MB
staging write roughly every 12 blocks of travel, and the prediction is that it costs the same in
all four scenes — which is falsifiable with `renderClouds:"fast"`.

**Consequence for this mod:** the sort and draw-list changes below are real but small, and they
are throughput improvements. The 1% lows are a *stall* problem with a different fix list, and it
starts with instrumentation rather than with a patch. Naming them the same project would hide
that.
