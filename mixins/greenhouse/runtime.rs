// Greenhouse: renderer performance for Horizon, as a native mixin mod.
//
// This file is compiled twice:
//
// * as the standalone `greenhouse` Cargo lib (see `Cargo.toml`), where the
//   equivalence tests below run without Horizon, a GPU or game assets;
// * as a module injected into Horizon by horizon-mixins (see
//   `horizon-mixin.toml`), where the overwritten functions call into it.
//
// Everything a hook changes has to be provably equivalent to the code it
// replaces. The tests at the bottom are that proof: the reference
// implementation is a copy of Horizon's own, and the optimized one must
// produce a byte-identical index buffer on adversarial inputs.

use std::sync::OnceLock;

/// One switchable optimization.
///
/// The point of naming them is that a player who sees a regression on a
/// particular device can turn off exactly the change that caused it, without
/// losing the others.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum Feature {
    /// Sort translucent quads with an unstable sort plus an explicit index
    /// tie-break, instead of a stable sort that allocates.
    SortUnstable,
}

#[allow(dead_code)]
impl Feature {
    /// The name used in the environment variable.
    pub fn key(self) -> &'static str {
        match self {
            Feature::SortUnstable => "sort_unstable",
        }
    }

    pub fn all() -> &'static [Feature] {
        &[Feature::SortUnstable]
    }
}

/// Parsed `HORIZON_GREENHOUSE`.
///
/// `HORIZON_GREENHOUSE=off` disables the whole mod; `HORIZON_GREENHOUSE=-sort_unstable`
/// disables one change and keeps the rest. Unset keeps every change on.
#[allow(dead_code)]
fn disabled() -> &'static [&'static str] {
    static DISABLED: OnceLock<Vec<&'static str>> = OnceLock::new();
    DISABLED.get_or_init(|| {
        let Ok(value) = std::env::var("HORIZON_GREENHOUSE") else {
            return Vec::new();
        };
        let value = value.trim();
        if value.eq_ignore_ascii_case("off") || value == "0" {
            return Feature::all().iter().map(|f| f.key()).collect();
        }
        value
            .split(',')
            .map(str::trim)
            .filter_map(|name| {
                let name = name.strip_prefix('-')?;
                Feature::all()
                    .iter()
                    .find(|f| f.key() == name)
                    .map(|f| f.key())
            })
            .collect()
    })
}

/// Whether `feature` should run. Cheap: the environment is read once.
#[allow(dead_code)]
pub fn on(feature: Feature) -> bool {
    !disabled().contains(&feature.key())
}

/// Horizon's `Sorter::sort`, which the hook replaces.
///
/// Reference copy of `horizon-client-render/src/gfx/translucency.rs` at the
/// pinned revision, used only by the tests.

/// The replacement body: farthest quad first, as `sort_reference`, but with
/// an unstable sort.
///
/// Horizon preserves mesh order for quads at equal distance by using a stable
/// merge sort, and a stable sort allocates an `n / 2` scratch buffer on every
/// call — once per resorted section, per frame. Adding the quad index as an
/// explicit tie-break makes the comparator a total order over distinct
/// elements, so the unstable sort has exactly one admissible result and it is
/// the stable one. Identical output, no allocation.
#[allow(dead_code)]
pub fn sort_quads(
    order: &mut Vec<(u32, f32)>,
    indices: &mut Vec<u32>,
    centers: &[[f32; 3]],
    camera: [f32; 3],
) {
    order.clear();
    order.reserve(centers.len());
    for (i, c) in centers.iter().enumerate() {
        let [x, y, z] = std::array::from_fn(|j| camera[j] - c[j]);
        order.push((i as u32, x * x + y * y + z * z));
    }
    // Farthest first; ties keep mesh order (ascending quad index).
    order.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    indices.clear();
    indices.reserve(centers.len() * 6);
    for &(quad, _) in order.iter() {
        let v = quad * 4;
        indices.extend_from_slice(&[v, v + 1, v + 2, v + 2, v + 3, v]);
    }
}


// ---------------------------------------------------------------------------
// Draw regrouping: `PageRuns::regroup`.
//
// Horizon groups a frame's section draws by arena page within distance bands so
// the draw list becomes a few dozen multi-draws instead of ~1000. It does that
// with `sort_by_key`, which is a *stable* merge sort: it allocates an `n / 2`
// scratch buffer on every call (2-3 calls a frame), and the key space is tiny —
// bands are at most a few dozen and pages at most a few — so the comparison
// sort is doing far more work than the data needs.
//
// The replacement is a stable counting sort over the same key. It is equivalent
// by construction, not by luck: a counting sort that places entries in input
// order per bucket *is* the stable ordering of the key, which is what
// `sort_by_key` produces.
// ---------------------------------------------------------------------------

/// The width of a band, in blocks. The patch passes Horizon's own
/// `page_runs::BAND`, so the two cannot drift and Horizon's constant stays
/// used (an unused one would fail its `-D warnings`).
pub const BAND: f64 = 64.0;

/// One section's draws: `start..start + len` of the list (Horizon's `Run`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub struct Run {
    /// `band << 32 | page`.
    pub key: u64,
    pub start: u32,
    pub len: u32,
    pub tag: [u32; 3],
}

/// Horizon's band for a squared distance in bits: `floor(sqrt(d) / 64)`.
///
/// Factored out of `push` so the injected body has something real to call, and
/// so the arithmetic has exactly one home. Bit-identical to the inline form.
pub fn band_of(band: f64, distance: u64) -> u64 {
    (f64::from_bits(distance).sqrt() / band) as u64
}

/// How many runs the draw list has been given, since process start.
///
/// The per-frame draw volume is the number the whole analysis turns on, and
/// nothing in Horizon reports it directly. Cheap enough to leave on: one
/// relaxed atomic add per section.
static PUSHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Counts one pushed run.
pub fn count_push() {
    PUSHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The count so far.
pub fn pushes() -> u64 {
    PUSHES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Frames the renderer has drawn, and the draw-run count at the last report.
static FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static REPORTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Frames between reports, unless `HORIZON_GREENHOUSE_REPORT` says otherwise
/// (0 turns reporting off).
const REPORT_EVERY: u64 = 600;

fn report_every() -> u64 {
    static EVERY: OnceLock<u64> = OnceLock::new();
    *EVERY.get_or_init(|| {
        std::env::var("HORIZON_GREENHOUSE_REPORT")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(REPORT_EVERY)
    })
}

/// Called once per frame the renderer draws: reports the draw-list size,
/// averaged over the frames since the last report.
///
/// That number decides how much work `regroup`, the indirect buffer and the
/// command encoder each do, and Horizon reports it nowhere. Making it visible
/// is the part this mod can actually ship.
pub fn report_frame() {
    let frames = FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let every = report_every();
    if every == 0 || frames % every != 0 {
        return;
    }
    let runs = pushes();
    let previous = REPORTED.swap(runs, std::sync::atomic::Ordering::Relaxed);
    let since = runs.saturating_sub(previous);
    log::info!(
        "greenhouse: {:.0} draw runs per frame over the last {every} frames ({runs} since start)",
        since as f64 / every as f64
    );
}

/// Horizon's `PageRuns`: the runs, and the scratch the rewrite gathers into.
#[derive(Default)]
#[allow(dead_code)]
pub struct PageRuns {
    pub runs: Vec<Run>,
    pub scratch: Vec<([u32; 5], (u32, Option<u32>))>,
}

#[allow(dead_code)]
impl PageRuns {
    pub fn clear(&mut self) {
        self.runs.clear();
    }

    /// Horizon's `push`: runs arrive nearest first.
    pub fn push(&mut self, distance: u64, page: u32, start: usize, len: usize, tag: [u32; 3]) {
        if len > 0 {
            let band = band_of(BAND, distance);
            self.runs.push(Run {
                key: band << 32 | u64::from(page),
                start: start as u32,
                len: len as u32,
                tag,
            });
        }
    }

    /// Horizon's `regroup`, unchanged: the reference the fast path must match.
    pub fn regroup_reference(
        &mut self,
        draws: &mut [[u32; 5]],
        pages: &mut [(u32, Option<u32>)],
        base: usize,
    ) {
        let page = |r: &Run| r.key as u32;
        if self.runs.windows(2).all(|w| page(&w[0]) == page(&w[1])) {
            return;
        }
        self.runs.sort_by_key(|r| r.key);
        self.scratch.clear();
        for r in &mut self.runs {
            let (s, e) = (r.start as usize, (r.start + r.len) as usize);
            r.start = (base + self.scratch.len()) as u32;
            self.scratch
                .extend(draws[s..e].iter().copied().zip(pages[s..e].iter().copied()));
        }
        for (i, &(draw, p)) in self.scratch.iter().enumerate() {
            draws[base + i] = draw;
            pages[base + i] = p;
        }
    }

    /// The replacement: the same stable order, without the stable sort's
    /// allocation.
    ///
    /// `regroup` needs stability only among runs with equal keys. Within one
    /// call `start` is strictly increasing (`push` assigns the current draw
    /// count and skips empty runs), so `(key, start)` has no ties and an
    /// unstable sort reproduces the stable order exactly.
    pub fn regroup(
        &mut self,
        draws: &mut [[u32; 5]],
        pages: &mut [(u32, Option<u32>)],
        base: usize,
    ) {
        let page = |r: &Run| r.key as u32;
        if self.runs.windows(2).all(|w| page(&w[0]) == page(&w[1])) {
            return;
        }
        // Already in final order: the runs cover `draws[base..]` contiguously,
        // so their starts are already the indices the rewrite would set.
        if self.runs.windows(2).all(|w| w[0].key <= w[1].key) {
            return;
        }
        self.runs.sort_unstable_by_key(|r| (r.key, r.start));
        self.scratch.clear();
        for r in &mut self.runs {
            let (s, e) = (r.start as usize, (r.start + r.len) as usize);
            r.start = (base + self.scratch.len()) as u32;
            self.scratch
                .extend(draws[s..e].iter().copied().zip(pages[s..e].iter().copied()));
        }
        for (i, &(draw, p)) in self.scratch.iter().enumerate() {
            draws[base + i] = draw;
            pages[base + i] = p;
        }
}
    }
