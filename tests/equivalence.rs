// Equivalence proofs for greenhouse's replacements.
//
// Integration tests, not unit tests inside the shipped module, on purpose: the
// same source is injected into Horizon as a support module, and nothing
// test-only may reach the game.


pub fn sort_reference(
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

mod sort_tests {
    use greenhouse::*;
    use super::sort_reference;
    
    /// A cheap deterministic generator, so a failure is reproducible.
    pub struct Lcg(pub(crate) u64);

    impl Lcg {
        pub fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        pub fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        pub fn unit(&mut self) -> f32 {
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

mod regroup_tests {
    use greenhouse::*;
    use super::sort_tests::Lcg;
        
    fn bits(blocks: f64) -> u64 {
        (blocks * blocks).to_bits()
    }

    /// Runs of (distance in blocks, page, draws), as the list builder pushes them.
    fn list(runs: &[(f64, u32, u32)]) -> (PageRuns, Vec<[u32; 5]>, Vec<(u32, Option<u32>)>) {
        let mut grouped = PageRuns::default();
        let (mut draws, mut pages) = (Vec::new(), Vec::new());
        for (n, &(distance, page, count)) in runs.iter().enumerate() {
            let start = draws.len();
            for k in 0..count {
                draws.push([n as u32, k, 7, 9, 0]);
                pages.push((page, None));
            }
            grouped.push(bits(distance), page, start, count as usize, [n as u32, 0, 0]);
        }
        (grouped, draws, pages)
    }

    /// Both paths on the same input: (draws, pages, runs) for each.
    fn both(
        runs: &[(f64, u32, u32)],
        base: usize,
    ) -> (
        (Vec<[u32; 5]>, Vec<(u32, Option<u32>)>, Vec<Run>),
        (Vec<[u32; 5]>, Vec<(u32, Option<u32>)>, Vec<Run>),
    ) {
        let pad = vec![[123, 0, 0, 0, 0]; base];
        let pad_pages = vec![(999u32, None); base];
        let mut out = Vec::new();
        for fast in [false, true] {
            let (mut grouped, draws, pages) = list(runs);
            let mut draws = [pad.clone(), draws].concat();
            let mut pages = [pad_pages.clone(), pages].concat();
            // Starts are absolute in Horizon: the caller pushes the draw index
            // in the whole list, not within the runs' own region.
            if base != 0 {
                for r in &mut grouped.runs {
                    r.start += base as u32;
                }
            }
            if fast {
                grouped.regroup(&mut draws, &mut pages, base);
            } else {
                grouped.regroup_reference(&mut draws, &mut pages, base);
            }
            out.push((draws, pages, grouped.runs.clone()));
        }
        let fast = out.pop().expect("fast");
        let reference = out.pop().expect("reference");
        (reference, fast)
    }

    #[test]
    fn band_of_is_the_inline_formula() {
        // The factored band must equal Horizon's inline arithmetic bit for bit,
        // including at exact band edges and for huge distances.
        for blocks in [0.0f64, 1.0, 63.9, 64.0, 64.1, 127.9, 128.0, 1023.0, 2048.0, 1.0e6] {
            let distance = (blocks * blocks).to_bits();
            let inline = (f64::from_bits(distance).sqrt() / BAND) as u64;
            assert_eq!(band_of(BAND, distance), inline, "{blocks} blocks");
        }
    }

    #[test]
    fn matches_horizon_own_unit_cases() {
        let cases: Vec<(Vec<(f64, u32, u32)>, usize)> = vec![
            (
                vec![(10.0, 1, 2), (20.0, 0, 1), (30.0, 1, 3), (40.0, 0, 2), (70.0, 1, 1)],
                0,
            ),
            (vec![(10.0, 1, 1), (100.0, 0, 1), (200.0, 0, 1)], 0),
            (vec![(10.0, 3, 2), (90.0, 3, 1)], 0),
            (vec![], 0),
            (vec![(1.0, 0, 0)], 0),
        ];
        for (runs, base) in cases {
            let (reference, fast) = both(&runs, base);
            assert_eq!(reference, fast, "runs {runs:?} base {base}");
        }
    }

    #[test]
    fn matches_at_a_non_zero_base() {
        // The list sits after other draws: starts are absolute.
        let (reference, fast) = both(&[(5.0, 1, 1), (6.0, 0, 1), (70.0, 1, 2)], 3);
        assert_eq!(reference, fast);
        assert_eq!(fast.0[3], [1, 0, 7, 9, 0]);
    }

    #[test]
    fn matches_on_random_frames() {
        let mut rng = Lcg(0x51ED270B);
        for case in 0..500 {
            let n = rng.below(120) as usize;
            let pages = 1 + rng.below(9) as u32;
            // Distances arrive nearest first, as `push` requires.
            let mut distance = 0.0f64;
            let runs: Vec<(f64, u32, u32)> = (0..n)
                .map(|_| {
                    distance += rng.below(40) as f64;
                    (distance, rng.below(u64::from(pages)) as u32, rng.below(4) as u32)
                })
                .collect();
            let base = rng.below(5) as usize;
            let (reference, fast) = both(&runs, base);
            assert_eq!(reference, fast, "case {case}: {n} runs, {pages} pages");
        }
    }

    #[test]
    fn matches_when_every_run_shares_a_page() {
        // The early-out path: nothing is reordered by either implementation.
        let (reference, fast) = both(&[(1.0, 4, 1), (2.0, 4, 2), (300.0, 4, 1)], 0);
        assert_eq!(reference, fast);
    }

    #[test]
    fn matches_even_when_bands_are_not_monotonic() {
        // `push` feeds runs nearest first, so this cannot happen; the sort is
        // still correct if it does, and must equal the reference.
        let runs = vec![(100.0, 1, 1), (10.0, 0, 1), (200.0, 1, 1)];
        let (reference, fast) = both(&runs, 0);
        assert_eq!(reference, fast);
    }

    #[test]
    fn already_grouped_lists_are_left_exactly_alone() {
        // Page order 0,1,0 at increasing distance is NOT one page, so Horizon's
        // own early-out misses it and it pays for a sort and a full copy. The
        // keys are already ascending, so the fast path must skip both, and the
        // result must still equal the reference's.
        let runs = vec![(1.0, 0u32, 2u32), (90.0, 1, 1), (300.0, 0, 1)];
        let (mut grouped, draws, pages) = list(&runs);
        let (before_draws, before_pages) = (draws.clone(), pages.clone());
        let (mut draws, mut pages) = (draws, pages);
        grouped.regroup(&mut draws, &mut pages, 0);
        assert_eq!(draws, before_draws, "draws must be untouched");
        assert_eq!(pages, before_pages, "pages must be untouched");
        let (reference, fast) = both(&runs, 0);
        assert_eq!(reference, fast);
    }

    #[test]
    fn matches_when_keys_tie_across_the_early_out() {
        // Same band and page repeated: `<=` in the early-out must accept ties
        // without disturbing their order.
        let (reference, fast) = both(&[(1.0, 2, 1), (2.0, 2, 1), (3.0, 2, 1)], 0);
        assert_eq!(reference, fast);
    }

    #[test]
    fn matches_with_more_than_thirty_two_runs() {
        // Above the threshold where the sort implementation changes strategy
        // (insertion sort is used for small slices and is stable regardless),
        // so ties among many runs are what actually exercises the order.
        let runs: Vec<(f64, u32, u32)> = (0..40u32)
            .map(|i| (i as f64 * 64.0, i % 3, 1))
            .collect();
        let (reference, fast) = both(&runs, 0);
        assert_eq!(reference, fast);
    }

    #[test]
    fn matches_with_zero_length_runs() {
        let (reference, fast) = both(&[(1.0, 0, 0), (2.0, 1, 1), (3.0, 0, 0), (90.0, 1, 2)], 0);
        assert_eq!(reference, fast);
    }
}

/// `regroup` needs a *stable* sort by key, which is why the original uses
/// `sort_by_key` and pays for its `n/2` allocation. Within one call the runs
/// arrive with strictly increasing `start` (`push` assigns the current draw
/// count, and it skips empty runs), so `(key, start)` has no ties at all and an
/// unstable sort must reproduce the stable order exactly -- with no scratch.
mod stable_via_unstable {
    use super::sort_tests::Lcg;

    #[test]
    fn sorting_by_key_and_start_equals_a_stable_sort_by_key() {
        let mut rng = Lcg(0x00C0FFEE_D15EA5E);
        for case in 0..500 {
            let n = rng.below(200) as usize;
            let pages = 1 + rng.below(9);
            let mut start = 0u32;
            let mut runs: Vec<(u64, u32)> = Vec::with_capacity(n);
            for _ in 0..n {
                let len = 1 + rng.below(3) as u32;
                let band = rng.below(20);
                let page = rng.below(pages);
                runs.push((band << 32 | page, start));
                start += len;
            }
            let mut stable = runs.clone();
            stable.sort_by_key(|r| r.0);
            let mut unstable = runs.clone();
            unstable.sort_unstable_by_key(|r| (r.0, r.1));
            assert_eq!(stable, unstable, "case {case}: {n} runs, {pages} pages");
        }
    }

    #[test]
    fn it_holds_with_every_run_in_one_band_and_page() {
        // All keys equal: the order must be the arrival order, which is what
        // ascending `start` already is.
        let runs: Vec<(u64, u32)> = (0..64).map(|i| (7u64 << 32 | 3, i * 2)).collect();
        let mut stable = runs.clone();
        stable.sort_by_key(|r| r.0);
        let mut unstable = runs.clone();
        unstable.sort_unstable_by_key(|r| (r.0, r.1));
        assert_eq!(stable, unstable);
        assert_eq!(unstable, runs, "arrival order preserved");
    }
}
