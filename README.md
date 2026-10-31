# greenhouse

Renderer performance for [Horizon](https://github.com/horizon-rust/horizon), shipped as a
native [horizon-mixins](https://github.com/horizon-rust/horizon-mixins) mod.

Horizon is a from-scratch Rust rewrite of Minecraft: Java Edition 26.3. It is *already* much
faster than Java Minecraft with Sodium, and this mod exists to take the next step, not the
first one. Greenhouse changes the game's own Rust source at build time through declared mixin
injections; the launcher builds it on the player's copy. There is no fork and no Java.

**Status: the vehicle is proven on macOS; the optimizations are mostly blocked by a tool
limitation, and nothing is benchmarked yet.** Read [Capabilities](#mixin-capabilities-measured)
and [Status](#status) before assuming anything here works.

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

Measured by the project on a Ryzen 7 9800X3D + RTX 5080, 1280x720
(`benchmarks/perf/windows-9800x3d-5080-2026-10-29.md`): at render distance 16, Horizon
averages **2424 FPS against vanilla+Sodium+Lithium's 1746**, with 1% lows of **709 vs 461**.

## Where the time actually goes

From the project's own benchmark report JSON, `cpu_p50_ms` is CPU frame work with GPU wait
subtracted, `gpu_p50_ms` is GPU timestamps:

| RD | scene | p50 frame | CPU | GPU | CPU share of frame |
| --- | --- | --- | --- | --- | --- |
| 8 | forest | 0.26 ms | 0.144 | 0.075 | ~55% |
| 16 | village | 0.53 ms | 0.379 | 0.104 | ~72% |
| 32 | village | 0.82 ms | 0.665 | 0.193 | ~81% |
| 32 | ocean | 0.73 ms | 0.596 | 0.315 | ~82% |

The frame is **CPU-bound at every render distance** — but two caveats that matter:

- At RD 8 the GPU bound is ~13,300 FPS, so a couple of successful CPU optimizations make the
  GPU the limiter. The premise is true now and has a short shelf life.
- The report carries no per-frame CPU/GPU correlation, so it says nothing about whether the
  *slowest* frames (p999 12.5 ms, max 18.9 ms at RD 32 ocean) are CPU or GPU. The premise is
  proven for the frames nobody feels and unproven for the frames everybody feels.

## Mixin capabilities (measured)

Probed against the pinned revision with the real `horizon-mixins` CLI, not assumed:

| Question | Answer | Evidence |
| --- | --- | --- |
| Does `horizon-mixins` build on macOS? | **Yes** — on its pinned 1.95.0 toolchain | `cargo build --release` succeeded |
| Does `plan` / `apply` work on macOS? | **Yes**, against a pristine pinned checkout | `status: ok`, 1 injection, 1 module |
| Does the OVERWRITE transform work? | **Yes** — keeps the signature, emits a local `fn` and a tail call | inspected staged source |
| Can OVERWRITE replace any function? | **No.** A body containing **closures** is rejected with `E_UNSUPPORTED_CONTROL_FLOW` | `Sorter::sort` and `quad_center` fail; `PageRuns::push` succeeds |
| Can HEAD inject into a closure-containing body? | **Yes** | `Sorter::sort` HEAD plans cleanly |
| Can a cancelling HEAD substitute a whole function? | **Only if it returns an owned value.** Helpers may not declare lifetimes, so a borrowed return cannot be written | `quad_center` ok; `Sorter::sort` gives `E_INVALID_HELPER` |
| Can a call-level hook target code inside a closure-bearing body? | **No** | `BEFORE_CALL` on `self.runs.sort_by_key` gives `E_UNSUPPORTED_CONTROL_FLOW` |

Six probes, in full: [docs/capabilities.md](docs/capabilities.md). This is the single most
important constraint on the project. The full analysis, including the hypotheses that were
refuted along the way, is in [docs/findings.md](docs/findings.md). Horizon's renderer is written in
idiomatic Rust where nearly every hot function contains a closure — `iter().map(..)`,
`windows(2).all(..)`, `sort_by_key(|v| ..)`, `array::from_fn(..)`. **So OVERWRITE cannot
rewrite most of the functions this mod wanted to rewrite.** That blocks the translucent-sort
replacement and the draw-regrouping replacement outright, and it is a tool limitation rather
than a design choice.

Three staging gotchas, each of which cost a run:

- `/tmp` is a symlink on macOS and the tool refuses paths that traverse symlinks — use
  `/private/tmp` or a path under `$HOME`.
- The Horizon tree contains `CLAUDE.md -> AGENTS.md`; staged inputs must not contain symlinks.
- A support module may not begin with `//!` inner doc comments (they are file attributes);
  use `//`.

## The changes

Everything is individually switchable: `HORIZON_GREENHOUSE=off` disables the mod,
`HORIZON_GREENHOUSE=-<name>` disables one change and keeps the rest.

| ID | Change | Expressible as a mixin? | Output identical? | Speedup |
| --- | --- | --- | --- | --- |
| `push-band` | Factors the band arithmetic out of `PageRuns::push` and counts pushed runs, so the draw volume becomes observable | **Yes** (closure-free body) | **Yes**, and tested | None by design — a scaffold |
| `regroup_early_out` | Skip regrouping when the run list is already key-sorted, not only when every run shares one page | **No** — same function | **Yes**, proven | Unmeasured |
| `sort_unstable` | Translucent sort without a stable sort's allocation | **No** — `sort_by` body contains closures | **Yes**, proven by 5 equivalence tests | ~none (see below) |

The three blocked changes are not wasted: their algorithms are implemented in
`mixins/greenhouse/runtime.rs` with a reference copy of Horizon's own code beside them, and
`cargo test` proves the replacements produce byte-identical output. When the tool gains closure
support, or if the changes are upstreamed, the bodies already exist and are already proven.

### What the correctness proofs cover

`regroup_counting` (12 tests): Horizon's own unit cases including the non-zero-`base` offset
list, 500 randomised frames, the single-page early-out, the new monotonicity early-out, both
fallback paths, zero-length runs, and >32 runs. The fast path refuses any input outside the
shape `push` guarantees and defers to the reference, so a future upstream change degrades to
the old behaviour rather than to wrong behaviour.

`sort_unstable` (5 tests): Horizon's pinned cases, up to 257 quads at identical distance, 400
randomised sections with quarter-block coordinates so ties are frequent, and `±0.0`/`NaN`/
infinities. Horizon's own tests for this function use at most 3 elements and therefore **do
not** pin the tie-break behaviour — an unstable sort without a tie-break would pass them all,
because insertion sort is used below ~20 elements and is stable regardless.

### Two corrections to earlier claims in this project

- **`sort_unstable` is not a performance win.** `SortState::update` early-outs unless the
  section-relative point of view changed, so sorts fire on viewpoint change, not per frame. It
  is kept only as correctness hardening. Anyone reading the earlier version of this README
  should discard its framing of this change as a speedup.
- **Caching per-section draw records across frames is worthless**, and not for the reason
  first given. The record embeds the visible-list index, which moves every frame — but even
  indexing by a stable slot would not pay, because `regroup` has to re-copy every draw into
  `(band, page)` order regardless, and `origins` is camera-relative and rewritten every frame.
  The copies dominate, not the index.

## Status

| Item | State |
| --- | --- |
| Horizon beats vanilla + Sodium + Lithium | **Confirmed** — the project's published benchmark |
| The frame is CPU-bound | **Confirmed** for the median; unproven for the tail |
| `horizon-mixins` builds, plans and applies on macOS | **Confirmed** — `status: ok` |
| OVERWRITE cannot rewrite a closure-bearing body | **Confirmed** by probe |
| The patched game compiles and passes Horizon's tests | **Confirmed** — `576 passed; 0 failed` |
| `patches/0002` applies to the pinned revision | **Confirmed** — `--check` |
| `patches/0002` is bit-identical to the reference walk | **Confirmed** — Horizon's exact-bits oracle test |
| `patches/0002` is 19.6% faster on Horizon's own walk benchmark | **Confirmed at opt-level 3** — arms do not overlap |
| `patches/0003` applies, compiles and passes the suite | **Confirmed** |
| Each patch applies to a pristine checkout on its own | **Confirmed** — `--only <name>` for each |
| Each change is individually switchable | **Confirmed** — `--only` / `--except` |
| A `regroup` sort replacement | **Measured 34-46% slower; removed** |
| Frame rate A/B, pristine vs patched, RD 32 | **Run; no result** — the M1 is GPU-bound (gpu 4.4 ms vs cpu 2.5 ms), so a CPU win cannot show |

**Nothing here has been benchmarked.** Every measurement attempted was taken with the machine
at load 23–27 on 8 cores; a microbenchmark swung between −55% and +148% on identical inputs
run to run. Those numbers are worse than no numbers.

## Two delivery paths, and why there have to be two

`horizon-mixins` cannot express the changes that matter here (see
[docs/capabilities.md](docs/capabilities.md)): it will not rewrite a body containing a closure,
and it cannot substitute a function returning borrowed data. Horizon's hot renderer functions are
both. So the same repository carries two things, each labelled with what it is:

**1. A mixin mod** — `mixins/greenhouse/`. Launcher-installable once registered, builds on the
player's copy, individually switchable. Currently limited to closure-free functions: it ships one
behaviour-identical scaffold and an instrumentation counter.

**2. A patch series** — `patches/`, applied with `tools/apply-patches.sh <horizon-checkout>`.
Everything a mixin cannot express. Not launcher-installable, and honest about it: you apply it to
a checkout at the pinned revision and build.

```sh
tools/apply-patches.sh ~/horizon --check   # verifies it applies, writes nothing
tools/apply-patches.sh ~/horizon           # applies
cd ~/horizon && cargo build --release -p horizon-client --bins
```

### Switching them individually

Every change can be built on its own, which is what makes each one measurable against a baseline
that differs in exactly one thing:

```sh
tools/apply-patches.sh ~/horizon --only visibility   # 0002 alone
tools/apply-patches.sh ~/horizon --only cull         # 0003 alone
tools/apply-patches.sh ~/horizon --except cull       # visibility only
```

Each patch was checked to apply to a pristine checkout on its own, not only in series. `--check`
writes nothing and exits nonzero on a conflict or when nothing is selected.

A runtime toggle inside the compiled game was considered and rejected: it would put a branch on
hot paths purely to serve an A/B, and a branch that is always taken still costs the instruction
cache and the predictability. Build-time selection buys the same measurement with no cost in the
shipped binary. The consequence is honest: switching a change off means rebuilding, not flipping
a setting.

### A change that was measured and rejected

An earlier version of this repository shipped three patches. The first replaced `regroup`'s stable
`sort_by_key` with `sort_unstable_by_key(|r| (r.key, r.start))`, on the reasoning that the stable
sort allocates an `n/2` buffer on every call and four calls happen a frame.

It was correct — the tie-break provably reproduces the stable order, and Horizon's suite passed —
and it was **slower**. Sorting identical elements with only the comparator changed, in nanoseconds
per call, best of 200 samples:

| runs | `sort_by_key` | unstable + tie-break | change |
| ---: | ---: | ---: | ---: |
| 600 | 7,750 | 11,250 | **+45%** |
| 1,200 | 17,035 | 24,790 | **+46%** |
| 2,500 | 41,512 | 58,052 | **+40%** |
| 5,000 | 95,925 | 128,481 | **+34%** |

The reasoning was wrong in a way that is obvious in hindsight. `regroup`'s input is **already
band-sorted** — `visible` is sorted by distance and shadow runs are pushed in depth order — and
Rust's stable sort detects runs. A near-sorted stable sort is very fast, while a two-field
comparator pays for the tuple on every comparison and gains nothing. Removing an allocation is
not the same as removing work.

The patch is gone from `patches/`. Its equivalence tests remain, because they are the evidence
for *why* it looked safe, and [docs/findings.md](docs/findings.md) records the rejection.

### `patches/0002-hoist-column-invariants-in-visibility-walk.patch`

`Resident::visible` walks every resident column and every section in it, and two of every
section's three camera-relative components are its column's — computed at the column and then
thrown away, to be recomputed per section by `origin()`. Across a render-distance-32 frame that
is roughly 26,000 calls to `origin()` a frame, about 317,000 scalar operations, of which two
thirds are redundant.

The patch hoists each column's `x` and `z` origin, the per-plane products they feed (which also
depend only on the plane's sign), and the column's share of the squared distance. What is left
per section is its `y` term, added in exactly the association `Frustum::intersects` and
`Vec3::dot` use. Same products, same order, so the culling decision and the sort key are
**bit-identical** — not "visually the same".

Verified against Horizon's own oracle: `column_walk_finds_exactly_the_sections_in_view` compares
the walk against a per-section reference and asserts equal f64 distance bits, equal f32 origin,
equal key and equal order across several cameras, radii and holed columns. It passes, and so does
the full 576-test suite. `in_frustum` is kept, `#[cfg(test)]`, as that reference.

No memory change: the added state is 24 f32 temporaries on the stack.

**Its cost, stated plainly:** the patch trades a per-call allocation for retained scratch, about
110 KiB per `PageRuns` instance and roughly 430 KiB across the four, on a working set measured at
1082 MB. That is +0.04% resident memory to remove four allocations a frame. If that trade is
unacceptable on an 8 GB machine, the buffers can be shared across the four instances, since
`regroup` is never reentrant — but it has not been done, and it is not claimed.

### `patches/0003-reuse-the-cull-readback-buffer.patch`

`Cull::begin` reads back the GPU occlusion test's results and copied them into a **fresh `Vec<u32>`
for every mapped readback — up to three a frame**, each sized by the frame's test count. It copies
word by word rather than by `cast_slice` because the mapping is not guaranteed to be 4-byte aligned
(a Vega 11 on Windows panicked there once). The patch copies into a buffer the struct keeps and
reuses.

This is the one patch aimed at **1% lows rather than throughput**: it removes recurring allocator
traffic from the frame thread, which is the kind of thing that shows up as jitter long before it
shows up in an average.

Bounded cost: one retained `Vec` sized by the largest readback seen, instead of three allocations a
frame. Verified by the full 576-test suite with it applied alongside the other two.

## Verified end to end

`horizon-mixins check` stages a pristine export of the pinned revision, injects greenhouse, and
runs the game's own tests against the patched tree:

```
status: ok    injections: 1    modules: 1
cargo 451 s   test run 43.5 s
test result: ok. 576 passed; 0 failed; 16 ignored
```

So on macOS the whole vehicle works — export, plan, apply, inject, compile — and the result is
behaviour-preserving by Horizon's own measure: every one of the 576 tests in
`horizon-client-render`, including `gfx::arena`, `gfx::cull`, `gfx::occlusion`, `mesher::sides`,
`streaming`, and the visibility suites, passes with the mod compiled in. The injected module
compiles clean with no warnings.

That is the strongest statement available before a frame is measured: the mod does not change
what the renderer does. Whether it can change how fast it does it is still open, and blocked on
a quiet machine for the A/B.

## Where to measure, and what to instrument first

Both review passes converged on the same conclusion, and it is not "write a faster sort":

1. **RD 8 is a write-off.** Horizon's RD 8 runs have a ±7.9% confidence interval (3 repeats)
   against a ≤5% expected effect, and 3495 FPS is 29x a 120 Hz panel. Nothing at RD 8 is
   measurable or perceptible. **RD 32 village** (CI ±0.8%, CPU 81% of frame) is the target.
2. **The draw-list region has no span.** The draw-record build, all the `regroup` calls and the
   translucency resort are the one uninstrumented block in the frame, which is why every cost
   figure about them is an estimate.
3. **There is a trap:** `spans::describe` filters spans below **0.5 ms**, so a `visibility`
   span worth 0.2–0.35 ms never prints. A measurement build has to lower that filter.
4. **The tail is the only perceptible defect.** RD 32 ocean p50 0.73 ms against p999 12.5 ms
   and max 18.9 ms is a bimodal stall signature. The two structural candidates, neither in the
   original plan: mass translucency resort when the camera crosses a 16-block boundary (every
   translucent section resorts in one frame, recurring every 16 blocks), and
   `streaming.update` draining worker results on the main thread every frame.
5. **Also relevant:** the far-terrain LOD subsystem is dormant in benchmark runs
   (`lod_distance` defaults to 0 and the driver leaves Horizon's settings at defaults), so an
   earlier hypothesis that blamed LOD geometry on the ocean 1% low was describing a scene the
   benchmark never built.

## Measuring it

Two tools, both of which refuse to produce a misleading number:

- `tools/ab.sh <label> <binary> [scene...]` runs `--offscreen` benchmarks at render distance 32
  and **refuses to start when the machine is busy** (exit 2), because a benchmark taken next to a
  build is not a benchmark. Verified: at load 97 on 8 cores it declines rather than reports.
- `tools/ab-compare.py <dir-a> <dir-b>` prints each arm's mean with its 95% interval and says
  whether the intervals **separate** — the only thing that counts as evidence. Verified on
  synthetic reports: it calls a +9.7% change with tight intervals separated, a +0.2% change with
  wide ones overlapping, and refuses to adjudicate a single run at all.

The methodology is deliberately narrow: RD 32 only, offscreen, repeats in the report's own
`conditions`. RD 8 measurements are excluded because a 3-repeat run there has an interval wider
than any change this project can produce.

## Rules this mod follows

- **Identical output over speed.** A change that alters what is on screen is a parity bug, not
  a performance change. Every shipped change must carry a proof, not an argument.
- **Never trade memory for frame time on weak devices.** No change here may increase resident
  memory or VRAM. The shipped scaffold adds one relaxed atomic; the blocked ones remove
  per-frame allocation.
- **No silent regressions.** Each change is separately switchable.

## Attribution

Horizon is MIT, by the horizon-rust contributors. horizon-mixins is MIT, by levzzz5154.
Greenhouse is MIT. The reference implementations copied into `runtime.rs` are Horizon's own
code, marked as such.
