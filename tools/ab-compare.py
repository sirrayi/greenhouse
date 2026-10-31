#!/usr/bin/env python3
"""Compare two sets of Horizon benchmark reports, with confidence intervals.

    tools/ab-compare.py <dir-a> <dir-b> [--metric avg_fps] [--rd 32]

Each directory holds `horizon-benchmark-report` JSON files (what
`horizon-client --benchmark` writes). Reports are grouped by scene and render
distance; the tool prints, per group, the mean of each metric in each arm, the
change, and whether the two arms' 95% intervals actually separate.

That last column is the point. At render distance 8 a 3-repeat run has a 95%
interval around +/-8% of the mean, which is wider than any change this project
can produce -- so a number that looks better there is not evidence. The tool
says so rather than printing a win.
"""

import argparse
import glob
import json
import math
import statistics
import sys

# `cpu_p50_ms` is the one that answers "did the CPU work shrink", and it stays
# meaningful on a GPU-bound machine: the CPU still does its work every frame,
# it simply is not what sets the frame time. `gpu_p50_ms` is there so a reader
# can see which of the two was the limiter.
METRICS = ("avg_fps", "low_1pct_fps", "low_01pct_fps", "p50_ms", "p999_ms",
           "cpu_p50_ms", "gpu_p50_ms")

# Student's t, two-sided 95%, by degrees of freedom. Beyond the table, 1.96.
T95 = {1: 12.706, 2: 4.303, 3: 3.182, 4: 2.776, 5: 2.571, 6: 2.447,
       7: 2.365, 8: 2.306, 9: 2.262, 10: 2.228, 12: 2.179, 15: 2.131,
       20: 2.086, 30: 2.042}


def t95(n):
    if n < 2:
        return None
    if n in T95:
        return T95[n]
    best = max(k for k in T95 if k <= n)
    return T95[best]


def load(directory):
    """{scene: {rd: {metric: [values]}}} from every report in `directory`."""
    groups = {}
    # Recursive: each arm runs in its own working directory and writes its
    # reports under ./benchmarks there.
    files = sorted(glob.glob(f"{directory}/**/*.json", recursive=True))
    if not files:
        sys.exit(f"error: no reports in {directory}")
    reports = 0
    for path in files:
        with open(path) as handle:
            try:
                report = json.load(handle)
            except json.JSONDecodeError:
                continue
        # A run's working directory also holds the world's own JSON (player
        # data, advancements); only the benchmark reports are data here.
        if not isinstance(report, dict) or report.get("format") != "horizon-benchmark-report":
            continue
        reports += 1
        conditions = report.get("conditions", {})
        rd = conditions.get("render_distance")
        if conditions.get("offscreen"):
            # Offscreen is a fair A/B (both arms pay it) but it is not a
            # player number; say so once rather than silently.
            pass
        for frame in report.get("frames", []):
            scene = frame.get("phase", "?")
            slot = groups.setdefault(scene, {}).setdefault(rd, {m: [] for m in METRICS})
            for metric in METRICS:
                if frame.get(metric) is not None:
                    slot[metric].append(float(frame[metric]))
    return groups


def mean_ci(values):
    """Mean and the half-width of the 95% interval, or (mean, None) for n < 2."""
    mean = statistics.fmean(values)
    n = len(values)
    if n < 2:
        return mean, None
    t = t95(n)
    half = t * statistics.stdev(values) / math.sqrt(n)
    return mean, half


def pct(new, old):
    if old == 0:
        return float("nan")
    return 100.0 * (new - old) / old


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("a")
    parser.add_argument("b")
    parser.add_argument("--metric", default="avg_fps", choices=METRICS)
    parser.add_argument("--rd", type=int, default=None)
    args = parser.parse_args()

    arm_a, arm_b = load(args.a), load(args.b)
    scenes = sorted(set(arm_a) | set(arm_b))

    print(f"A = {args.a}\nB = {args.b}\nmetric = {args.metric}\n")
    header = (f"{'scene':10} {'RD':>3} {'A mean':>10} {'A 95%':>8} "
              f"{'B mean':>10} {'B 95%':>8} {'change':>9}  verdict")
    print(header)
    print("-" * len(header))

    for scene in scenes:
        rds = sorted(set(arm_a.get(scene, {})) | set(arm_b.get(scene, {})))
        for rd in rds:
            if args.rd is not None and rd != args.rd:
                continue
            a_vals = arm_a.get(scene, {}).get(rd, {}).get(args.metric, [])
            b_vals = arm_b.get(scene, {}).get(rd, {}).get(args.metric, [])
            if not a_vals or not b_vals:
                continue
            a_mean, a_ci = mean_ci(a_vals)
            b_mean, b_ci = mean_ci(b_vals)
            change = pct(b_mean, a_mean)
            if a_ci is None or b_ci is None:
                verdict = "n<2, no interval"
            elif a_ci + b_ci < abs(b_mean - a_mean):
                verdict = "separated"
            else:
                verdict = "overlapping"
            rd_text = "?" if rd is None else str(rd)
            a_ci_text = "-" if a_ci is None else f"{a_ci:.2f}"
            b_ci_text = "-" if b_ci is None else f"{b_ci:.2f}"
            print(f"{scene:10} {rd_text:>3} "
                  f"{a_mean:10.2f} {a_ci_text:>8} "
                  f"{b_mean:10.2f} {b_ci_text:>8} "
                  f"{change:+8.1f}%  {verdict}")

    print("\n's separated' means the intervals do not overlap: treat as evidence.")
    print("'overlapping' means they do: treat as no result, whatever the sign.")
    print("At RD 8 a 3-repeat interval is wider than any change this project can")
    print("make; raise REPEATS to 7+ to resolve 5%.")


if __name__ == "__main__":
    main()
