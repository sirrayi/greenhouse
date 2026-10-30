//! Greenhouse: renderer performance for Horizon, as a native mixin mod.
//!
//! This file is compiled twice:
//!
//! * as the standalone `greenhouse` Cargo lib (see `Cargo.toml`), where the
//!   equivalence tests below run without Horizon, a GPU or game assets;
//! * as a module injected into Horizon by horizon-mixins (see
//!   `horizon-mixin.toml`), where the overwritten functions call into it.
//!
//! Everything a hook changes has to be provably equivalent to the code it
//! replaces. The tests at the bottom are that proof: the reference
//! implementation is a copy of Horizon's own, and the optimized one must
//! produce a byte-identical index buffer on adversarial inputs.

use std::sync::OnceLock;

/// One switchable optimization.
///
/// The point of naming them is that a player who sees a regression on a
/// particular device can turn off exactly the change that caused it, without
/// losing the others.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feature {
    /// Sort translucent quads with an unstable sort plus an explicit index
    /// tie-break, instead of a stable sort that allocates.
    SortUnstable,
}

impl Feature {
    /// The name used in the environment variable.
    pub fn key(self) -> &'static str {
        match self {
            Feature::SortUnstable => "sort_unstable",
        }
    }

    fn all() -> &'static [Feature] {
        &[Feature::SortUnstable]
    }
}

/// Parsed `HORIZON_GREENHOUSE`.
///
/// `HORIZON_GREENHOUSE=off` disables the whole mod; `HORIZON_GREENHOUSE=-sort_unstable`
/// disables one change and keeps the rest. Unset keeps every change on.
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
pub fn on(feature: Feature) -> bool {
    !disabled().contains(&feature.key())
}

/// Horizon's `Sorter::sort`, which the hook replaces.
///
/// Reference copy of `horizon-client-render/src/gfx/translucency.rs` at the
/// pinned revision, used only by the tests.
#[cfg(test)]
fn sort_reference(
    order: &mut Vec<(u32, f32)>,
    indices: &mut Vec<u32>,
    centers: &[[f32; 3]],
    camera: [f32; 3],
) {
    order.clear();
    order.extend(centers.iter().enumerate().map(|(i, c)| {
        let [x, y, z] = std::array::from_fn(|j| camera[j] - c[j]);
        (i as u32, x * x + y * y + z * z)
    }));
    // VertexSorting uses a stable merge sort, preserving mesh order for ties.
    order.sort_by(|a, b| b.1.total_cmp(&a.1));
    indices.clear();
    for &(quad, _) in order.iter() {
        let v = quad * 4;
        indices.extend([v, v + 1, v + 2, v + 2, v + 3, v]);
    }
}

/// The replacement body: farthest quad first, as `sort_reference`, but with
/// an unstable sort.
///
/// Horizon preserves mesh order for quads at equal distance by using a stable
/// merge sort, and a stable sort allocates an `n / 2` scratch buffer on every
/// call — once per resorted section, per frame. Adding the quad index as an
/// explicit tie-break makes the comparator a total order over distinct
/// elements, so the unstable sort has exactly one admissible result and it is
/// the stable one. Identical output, no allocation.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A cheap deterministic generator, so a failure is reproducible.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        fn unit(&mut self) -> f32 {
            // Quarter-block steps over a section-sized box: ties happen often.
            (self.below(64) as f32) / 4.0
        }
    }

    fn both(
        centers: &[[f32; 3]],
        camera: [f32; 3],
    ) -> (Vec<u32>, Vec<u32>) {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let mut fast_order = Vec::new();
        sort_reference(&mut a, &mut b, centers, camera);
        let mut fast_indices = Vec::new();
        sort_quads(&mut fast_order, &mut fast_indices, centers, camera);
        (b, fast_indices)
    }

    #[test]
    fn identical_on_the_existing_unit_cases() {
        // The three cases pinned by Horizon's own translucency tests.
        assert_eq!(
            both(
                &[[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [4.0, 0.0, 0.0]],
                [0.0; 3]
            )
            .0,
            [8, 9, 10, 10, 11, 8, 0, 1, 2, 2, 3, 0, 4, 5, 6, 6, 7, 4]
        );
        assert!(both(&[], [0.0; 3]).0.is_empty());
        assert_eq!(
            both(&[[0.5, 0.5, 0.0], [0.5, 0.5, 1.0]], [0.5, 0.5, -2.0]).0,
            [4, 5, 6, 6, 7, 4, 0, 1, 2, 2, 3, 0]
        );
    }

    #[test]
    fn identical_under_massive_distance_ties() {
        // Every centre the same: the tie-break alone decides the order.
        for n in [1, 2, 3, 7, 64, 257] {
            let centers = vec![[1.5, 2.5, 3.5]; n];
            let (reference, fast) = both(&centers, [1.5, 2.5, 3.5]);
            assert_eq!(reference, fast, "{n} identical centres");
        }
    }

    #[test]
    fn identical_on_random_sections_with_frequent_ties() {
        let mut rng = Lcg(0x9E3779B97F4A7C15);
        for case in 0..400 {
            let n = rng.below(300) as usize;
            let centers: Vec<[f32; 3]> =
                (0..n).map(|_| [rng.unit(), rng.unit(), rng.unit()]).collect();
            let camera = [rng.unit(), rng.unit(), rng.unit()];
            let (reference, fast) = both(&centers, camera);
            assert_eq!(reference, fast, "case {case}, {n} quads");
        }
    }

    #[test]
    fn identical_on_degenerate_inputs() {
        // Zero, negative zero, one quad, and NaN-bearing centres: `total_cmp`
        // is a total order, so both sorts must agree whatever the values.
        let cases: Vec<Vec<[f32; 3]>> = vec![
            vec![],
            vec![[0.0, 0.0, 0.0]],
            vec![[-0.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
            vec![[f32::NAN, 0.0, 0.0], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
            vec![[f32::INFINITY, 0.0, 0.0], [f32::NEG_INFINITY, 0.0, 0.0]],
        ];
        for centers in cases {
            let (reference, fast) = both(&centers, [0.0; 3]);
            assert_eq!(reference, fast);
        }
    }

    #[test]
    fn every_switch_can_be_turned_off_by_name() {
        // The parse is exercised through a copy: `disabled()` memoizes the
        // real environment once, which is what the game wants.
        assert!(Feature::all().iter().any(|f| f.key() == "sort_unstable"));
        assert_eq!(Feature::SortUnstable.key(), "sort_unstable");
    }

    /// What the change is worth, in isolation: ns per sort, stable vs unstable,
    /// at the section sizes Horizon actually sorts.
    ///
    /// `cargo test --release -- --ignored --nocapture` prints the table.
    /// Deliberately not part of the correctness suite: it is a microbenchmark,
    /// and its numbers belong in a report, not in a pass/fail assertion.
    #[test]
    #[ignore = "microbenchmark: run with --release --ignored --nocapture"]
    fn namespace_sort_cost_by_section_size() {
        use std::hint::black_box;
        use std::time::Instant;

        let mut rng = Lcg(0xDEADBEEFCAFEF00D);
        println!("\n  quads   stable ns/op   unstable ns/op    change");
        for n in [8usize, 32, 64, 128, 256, 512, 1024] {
            let centers: Vec<[f32; 3]> = (0..n)
                .map(|_| [rng.unit(), rng.unit(), rng.unit()])
                .collect();
            let camera = [7.5, 8.5, 7.5];
            let iters = 20_000 / (n / 8).max(1);

            let (mut order, mut indices) = (Vec::new(), Vec::new());
            // Warm both paths so the first call does not pay for growth.
            sort_reference(&mut order, &mut indices, &centers, camera);
            sort_quads(&mut order, &mut indices, &centers, camera);

            let mut time = |f: &dyn Fn(&mut Vec<(u32, f32)>, &mut Vec<u32>)| {
                let start = Instant::now();
                for _ in 0..iters {
                    f(black_box(&mut order), black_box(&mut indices));
                }
                start.elapsed().as_nanos() as f64 / iters as f64
            };
            let stable = time(&|o, i| sort_reference(o, i, black_box(&centers), camera));
            let unstable = time(&|o, i| sort_quads(o, i, black_box(&centers), camera));
            println!(
                "  {n:5}   {stable:10.1}   {unstable:13.1}   {:+.1}%",
                100.0 * (unstable - stable) / stable
            );
        }
        println!();
    }
}
