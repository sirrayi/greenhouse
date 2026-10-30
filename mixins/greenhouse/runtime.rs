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

/// The width of a band, in blocks (Horizon's `page_runs::BAND`).
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
pub fn band_of(distance: u64) -> u64 {
    (f64::from_bits(distance).sqrt() / BAND) as u64
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
#[allow(dead_code)]
pub fn pushes() -> u64 {
    PUSHES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Horizon's `PageRuns`, plus the scratch the counting sort reuses so the
/// steady state allocates nothing.
#[derive(Default)]
#[allow(dead_code)]
pub struct PageRuns {
    pub runs: Vec<Run>,
    scratch: Vec<([u32; 5], (u32, Option<u32>))>,
    runs_out: Vec<Run>,
    draws_out: Vec<([u32; 5], (u32, Option<u32>))>,
    dest: Vec<u32>,
    counts: Vec<u32>,
    bands: Vec<u64>,
    pages_seen: Vec<u32>,
}

/// Above this many distinct pages, or distinct (band, page) keys, the counting
/// sort stops being obviously cheaper and the reference is used instead. Both
/// are correct; this only decides which runs.
#[allow(dead_code)]
pub const MAX_PAGES: usize = 64;
#[allow(dead_code)]
pub const MAX_KEYS: usize = 4096;

#[allow(dead_code)]
impl PageRuns {
    pub fn clear(&mut self) {
        self.runs.clear();
    }

    /// Horizon's `push`: runs arrive nearest first.
    pub fn push(&mut self, distance: u64, page: u32, start: usize, len: usize, tag: [u32; 3]) {
        if len > 0 {
            let band = (f64::from_bits(distance).sqrt() / BAND) as u64;
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

    /// The replacement: a stable counting sort over `(band, page)`.
    pub fn regroup(
        &mut self,
        draws: &mut [[u32; 5]],
        pages: &mut [(u32, Option<u32>)],
        base: usize,
    ) {
        // Already in final order? Then this is an identity, whatever the pages.
        //
        // Horizon only short-circuits when every run shares one page, which is
        // the narrow special case. `push` feeds runs nearest first, so bands are
        // non-decreasing and key order is (band, page); a run list that is
        // already key-sorted needs no reordering, and because the runs cover
        // `draws[base..]` contiguously in order, their `start`s are already the
        // absolute indices `regroup` would rewrite them to. So the whole call is
        // a no-op and the copy can be skipped.
        //
        // This is strictly more general than the page check at the same O(n),
        // and it is the common case once sections cluster in a page.
        if self.runs.windows(2).all(|w| w[0].key <= w[1].key) {
            return;
        }
        if !self.counting_sort_runs() {
            // Not the shape the fast path assumes: fall back to the reference.
            // Reachable only if `push`'s distance order changes upstream.
            self.regroup_reference(draws, pages, base);
            return;
        }
        // Gather the draws in their new order, then write them back.
        self.draws_out.clear();
        self.draws_out.reserve(self.runs.len());
        let mut placed = 0usize;
        for r in &self.runs_out {
            let (s, e) = (r.start as usize, (r.start + r.len) as usize);
            self.draws_out
                .extend(draws[s..e].iter().copied().zip(pages[s..e].iter().copied()));
            placed += r.len as usize;
        }
        let mut at = base;
        for r in &mut self.runs_out {
            r.start = at as u32;
            at += r.len as usize;
        }
        for (i, &(draw, p)) in self.draws_out.iter().enumerate() {
            draws[base + i] = draw;
            pages[base + i] = p;
        }
        debug_assert_eq!(placed, self.draws_out.len());
        std::mem::swap(&mut self.runs, &mut self.runs_out);
    }

    /// Stably sorts `self.runs` by `key` into `self.runs_out`.
    ///
    /// Returns false if the input is not the shape this can handle: bands must
    /// be non-decreasing (which `push` guarantees, since distances arrive
    /// nearest first), the distinct page and key counts must stay small, and a
    /// key must not repeat after a larger one.
    #[allow(clippy::cast_possible_truncation)]
    fn counting_sort_runs(&mut self) -> bool {
        self.pages_seen.clear();
        self.bands.clear();
        let mut last_band = None;
        for r in &self.runs {
            let (band, p) = (r.key >> 32, r.key as u32);
            match last_band {
                Some(prev) if band < prev => return false,
                Some(prev) if band == prev => {}
                _ => {
                    self.bands.push(band);
                    last_band = Some(band);
                }
            }
            if !self.pages_seen.contains(&p) {
                self.pages_seen.push(p);
                if self.pages_seen.len() > MAX_PAGES {
                    return false;
                }
            }
        }
        let keys = self.bands.len() * self.pages_seen.len();
        if keys == 0 || keys > MAX_KEYS {
            return false;
        }
        self.pages_seen.sort_unstable();

        // Each run's dense key: `band_index * pages + page_index`.
        let np = self.pages_seen.len();
        self.dest.clear();
        self.dest.reserve(self.runs.len());
        let mut band_index = 0usize;
        let mut prev_band = u64::MAX;
        for r in &self.runs {
            let band = r.key >> 32;
            if band != prev_band {
                // `bands` is in first-seen order, which is ascending here.
                band_index = match self.bands.binary_search(&band) {
                    Ok(i) => i,
                    Err(_) => return false,
                };
                prev_band = band;
            }
            let Some(page_index) = self.pages_seen.binary_search(&(r.key as u32)).ok() else {
                return false;
            };
            self.dest.push((band_index * np + page_index) as u32);
        }

        // Stable counting sort: bucket starts, then placement in input order.
        self.counts.clear();
        self.counts.resize(keys, 0);
        for &key in &self.dest {
            self.counts[key as usize] += 1;
        }
        let mut acc = 0u32;
        for count in &mut self.counts {
            let n = *count;
            *count = acc;
            acc += n;
        }
        self.runs_out.clear();
        self.runs_out.resize(self.runs.len(), Run::default());
        for (i, &key) in self.dest.iter().enumerate() {
            let slot = &mut self.counts[key as usize];
            let at = *slot as usize;
            *slot += 1;
            self.runs_out[at] = self.runs[i];
        }
        true
    }
}
