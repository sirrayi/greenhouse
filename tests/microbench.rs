// Function-level microbenchmarks for the shipped patches.
//
// These are NOT frame rates and they do not replace a game benchmark. They
// measure the two functions the patches change, in isolation, so that the
// question "does this change do anything at all" has an answer that does not
// need a quiet machine.
//
// The estimator is the minimum over many samples rather than the mean. Under
// load a sample can be descheduled, which only ever makes it slower, so the
// minimum across enough samples approaches the uncontended time. The mean
// under load measures the machine's other work, not this code.
//
//   cargo test --release -- --ignored --nocapture microbench

use greenhouse::*;
use std::hint::black_box;
use std::time::Instant;

/// Best of `samples` timings of `reps` calls, in nanoseconds per call.
fn min_per_call(samples: usize, reps: usize, mut call: impl FnMut()) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..samples {
        let start = Instant::now();
        for _ in 0..reps {
            call();
        }
        let ns = start.elapsed().as_nanos() as f64 / reps as f64;
        if ns < best {
            best = ns;
        }
    }
    best
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A draws list and the runs over it, as one frame's solid list looks at RD 32.
fn frame(runs: usize, pages: u32, seed: u64) -> (Vec<(f64, u32, u32)>, Vec<[u32; 5]>, Vec<(u32, Option<u32>)>) {
    let mut rng = Lcg(seed);
    let mut spec = Vec::with_capacity(runs);
    let (mut draws, mut pages_out) = (Vec::new(), Vec::new());
    let mut distance = 0.0f64;
    for _ in 0..runs {
        distance += rng.below(8) as f64;
        let page = rng.below(u64::from(pages)) as u32;
        let count = 1 + rng.below(3) as u32;
        spec.push((distance, page, count));
        for k in 0..count {
            draws.push([0, k, 0, 0, 0]);
            pages_out.push((page, None));
        }
    }
    (spec, draws, pages_out)
}

fn build(spec: &[(f64, u32, u32)]) -> PageRuns {
    let mut runs = PageRuns::default();
    let mut at = 0usize;
    for &(distance, page, count) in spec {
        runs.push((distance * distance).to_bits(), page, at, count as usize, [0; 3]);
        at += count as usize;
    }
    runs
}

#[test]
#[ignore = "microbenchmark: cargo test --release -- --ignored --nocapture"]
fn microbench_regroup_old_vs_new() {
    println!("\n  regroup: per-call time, best of 200 samples of 20 calls");
    println!("  Both arms rebuild the runs and copy the draws inside the timed");
    println!("  region, so the difference is the sort and nothing else.");
    println!("  {:>6} {:>14} {:>14} {:>10}", "runs", "stable ns", "tie-break ns", "change");
    for n in [600usize, 1200, 2500, 5000] {
        let (spec, draws, pages) = frame(n, 6, 0x5EED);
        let mut a = PageRuns::default();
        let mut b = PageRuns::default();
        // The arms must do the same setup, so it is in both timings.
        let old = min_per_call(200, 20, || {
            let mut runs = build(&spec);
            let mut d = draws.clone();
            let mut p = pages.clone();
            runs.regroup_reference(&mut d, &mut p, 0);
            black_box((runs, d, p));
        });
        let new = min_per_call(200, 20, || {
            let mut runs = build(&spec);
            let mut d = draws.clone();
            let mut p = pages.clone();
            runs.regroup(&mut d, &mut p, 0);
            black_box((runs, d, p));
        });
        black_box((&mut a, &mut b));
        println!(
            "  {n:>6} {old:>14.0} {new:>14.0} {:>9.0}%",
            100.0 * (new - old) / old
        );
    }
}

/// The per-section arithmetic of the visibility walk, in both shapes.
///
/// `0002` hoists the column's x/z origin, its per-plane products and its share
/// of the squared distance. This measures the per-section remainder against the
/// whole, with the operands and the association `Frustum::intersects` and
/// `Vec3::length_squared` use -- `((x*x + y*y) + z*z)`, no FMA -- so the two
/// columns are doing the same arithmetic, differently arranged.
#[test]
#[ignore = "microbenchmark: cargo test --release -- --ignored --nocapture"]
fn microbench_visibility_old_vs_new() {
    // glam's Vec4::truncate().dot(v) is ((x*x + y*y) + z*z); same shape here.
    fn dot(p: [f32; 4], v: [f32; 3]) -> f32 {
        (p[0] * v[0] + p[1] * v[1]) + p[2] * v[2]
    }

    const PLANES: [[f32; 4]; 6] = [
        [0.9, 0.0, 0.4, 12.0],
        [-0.9, 0.0, 0.4, 12.0],
        [0.0, 1.0, 0.0, 40.0],
        [0.0, -1.0, 0.0, 40.0],
        [0.1, 0.1, 0.99, 30.0],
        [-0.1, -0.1, -0.99, 300.0],
    ];
    let camera = [37.5f64, 80.5, -19.5];
    // The real shape: ~4225 columns of ~19.6 tested sections at RD 32. The
    // hoisting is paid once per column, so it must be amortised over a
    // column's sections, not over the whole walk.
    let columns: Vec<(i32, i32)> = (0..200).map(|i| (i % 20 - 10, i / 20 - 5)).collect();
    let ys: Vec<i32> = (0..20).collect();
    // The real walk calls `reachable(key)` per section before the frustum
    // test: an out-of-line predicate over data the optimizer cannot see
    // through. Without it both arms get hoisted identically and the
    // comparison measures nothing.
    #[inline(never)]
    fn reachable(y: i32) -> bool {
        (black_box(y) & 3) != 0
    }

    let old = min_per_call(200, 20, || {
        let mut seen = 0u32;
        for &(cx, cz) in black_box(&columns) {
        for &y in black_box(&ys) {
            if !reachable(y) {
                continue;
            }
            // The original: the whole origin, per section, in f64 then f32.
            let ox = (f64::from(cx * 16) - camera[0]) as f32;
            let oy = (f64::from(-64 + y * 16) - camera[1]) as f32;
            let oz = (f64::from(cz * 16) - camera[2]) as f32;
            let (min, max) = ([ox, oy, oz], [ox + 16.0, oy + 16.0, oz + 16.0]);
            let mut visible = true;
            for p in &PLANES {
                let v = [
                    if p[0] >= 0.0 { max[0] } else { min[0] },
                    if p[1] >= 0.0 { max[1] } else { min[1] },
                    if p[2] >= 0.0 { max[2] } else { min[2] },
                ];
                if dot(*p, v) + p[3] < 0.0 {
                    visible = false;
                    break;
                }
            }
            if visible {
                seen += 1;
            }
            let dx = f64::from(cx * 16) - camera[0] + 8.0;
            let dy = f64::from(-64 + y * 16) - camera[1] + 8.0;
            let dz = f64::from(cz * 16) - camera[2] + 8.0;
            black_box((((dx * dx + dy * dy) + dz * dz)).to_bits());
        }
        }
        black_box(seen);
    });

    let new = min_per_call(200, 20, || {
        let mut seen = 0u32;
        let mut x_at = [0.0f32; 6];
        let mut z_at = [0.0f32; 6];
        for (i, p) in PLANES.iter().enumerate() {
            x_at[i] = if p[0] >= 0.0 { 16.0 } else { 0.0 };
            z_at[i] = if p[2] >= 0.0 { 16.0 } else { 0.0 };
        }
        for &(cx, cz) in black_box(&columns) {
        // The hoisting, paid once per column exactly as the walk pays it.
        let dx = f64::from(cx * 16) - camera[0];
        let dz = f64::from(cz * 16) - camera[2];
        let (ox, oz) = (dx as f32, dz as f32);
        let mut px_vx = [0.0f32; 6];
        let mut pz_vz = [0.0f32; 6];
        for (i, p) in PLANES.iter().enumerate() {
            px_vx[i] = p[0] * (ox + x_at[i]);
            pz_vz[i] = p[2] * (oz + z_at[i]);
        }
        let x8 = dx + 8.0;
        let z8 = dz + 8.0;
        let (xx, zz) = (x8 * x8, z8 * z8);
        for &y in black_box(&ys) {
            if !reachable(y) {
                continue;
            }
            let dy = f64::from(-64 + y * 16) - camera[1];
            let oy = dy as f32;
            let hi = oy + 16.0;
            let mut visible = true;
            for (i, p) in PLANES.iter().enumerate() {
                let vy = if p[1] >= 0.0 { hi } else { oy };
                if ((px_vx[i] + p[1] * vy) + pz_vz[i]) + p[3] < 0.0 {
                    visible = false;
                    break;
                }
            }
            if visible {
                seen += 1;
            }
            let y8 = dy + 8.0;
            black_box(((xx + y8 * y8) + zz).to_bits());
        }
        }
        black_box(seen);
    });

    println!("\n  visibility walk, 200 columns x 20 sections: best of 200 samples");
    println!("  original (per-section origin)  {old:>12.0} ns");
    println!("  hoisted (column invariants)    {new:>12.0} ns");
    println!("  change {:>+.0}%", 100.0 * (new - old) / old);
}

/// The sort change in patch 0001, isolated from `regroup`'s copies.
///
/// It is two changes at once — drop the stable sort's allocation, and add
/// `start` as a tie-break — so this measures them against each other directly
/// on the same key data, with the same amount of setup in both arms.
#[test]
#[ignore = "microbenchmark: cargo test --release -- --ignored --nocapture"]
fn microbench_sort_alone() {
    println!("\n  sort alone, best of 200 samples of 20 calls");
    println!("  {:>6} {:>16} {:>18} {:>10}", "runs", "sort_by_key ns", "unstable+start ns", "change");
    for n in [600usize, 1200, 2500, 5000] {
        let (spec, _, _) = frame(n, 6, 0xC0FFEE);
        // Both arms sort the SAME element type; only the comparator differs,
        // which is the whole of the change. Sorting different-sized elements
        // would measure memory traffic, not the sort.
        let mut at = 0u32;
        let mut runs: Vec<(u64, u32, [u32; 3])> = Vec::with_capacity(spec.len());
        for &(distance, page, count) in &spec {
            let band = (distance / 64.0) as u64;
            runs.push((band << 32 | u64::from(page), at, [0; 3]));
            at += count;
        }
        let old = min_per_call(200, 20, || {
            let mut v = black_box(runs.clone());
            v.sort_by_key(|r| black_box(r.0));
        });
        let new = min_per_call(200, 20, || {
            let mut v = black_box(runs.clone());
            v.sort_unstable_by_key(|r| black_box((r.0, r.1)));
        });
        println!(
            "  {n:>6} {old:>16.0} {new:>18.0} {:>9.0}%",
            100.0 * (new - old) / old
        );
    }
}
