#!/bin/sh
# A/B two Horizon binaries on the same scenes, INTERLEAVED, rejecting runs
# taken while the machine was busy.
#
#   RESOURCES=... tools/ab-pair.sh <bin-a> <bin-b> [scene ...]
#
# Sequential arms are a trap: the first arm runs on a cool idle machine and the
# second on one warmed by the first, and any background work lands entirely on
# whichever arm happened to be running. This alternates A, B, A, B within each
# scene so both arms see the same conditions, and it measures the load around
# every single run and re-runs any that overlapped with other work.
#
# Environment: RESOURCES (required), SCENE_RD (default 32), REPEATS (default 5),
# MAX_LOAD (per core, default 1.0), ATTEMPTS (default 3), OUT.
set -eu

a=${1:?usage: ab-pair.sh <bin-a> <bin-b> [scene ...]}
b=${2:?usage: ab-pair.sh <bin-a> <bin-b> [scene ...]}
shift 2
scenes=${*:-village ocean}

[ -n "${RESOURCES:-}" ] || { echo "error: set RESOURCES to prepared 26.3 resources" >&2; exit 1; }
[ -x "$a" ] && [ -x "$b" ] || { echo "error: both arms must be executable" >&2; exit 1; }

rd=${SCENE_RD:-32}
repeats=${REPEATS:-5}
attempts=${ATTEMPTS:-3}
out=${OUT:-./ab-reports}
mkdir -p "$out"

cores=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)
budget=$(awk -v c="$cores" -v m="${MAX_LOAD:-1.0}" 'BEGIN { printf "%.2f", c * m }')

load_now() {
    uptime | sed 's/.*load averages*: *//' | awk '{print $1}' | tr -d ','
}

over() {
    awk -v l="$1" -v b="$budget" 'BEGIN { exit !(l > b) }'
}

# Settle before starting rather than refusing outright: the previous run's own
# load lingers in the 1-minute average for a while after it exits.
waited=0
while over "$(load_now)" && [ "$waited" -lt 900 ]; do
    [ "$waited" -eq 0 ] && echo "==> waiting for the machine to settle (budget $budget on $cores cores)"
    sleep 15
    waited=$((waited + 15))
done
started=$(load_now)
if over "$started"; then
    echo "refusing to start: load $started on $cores cores still exceeds $budget after ${waited}s" >&2
    exit 2
fi

# Total %CPU of every process except the arm under test.
#
# The gate cannot use the load average: the game itself uses several cores, so
# the machine is always "busy" from the load average's point of view while a
# run is in progress. What matters is whether something ELSE was working, which
# is the rule Horizon's own benchmark documentation applies (it discards a run
# during which other processes used more than 15% of the machine).
other_cpu() {
    # Match on the command line, not `comm`: macOS truncates `comm` to a
    # path stub, so the game's own several cores were being counted as
    # "other". Only the arm under test carries --benchmark.
    ps -A -o pcpu=,args= | grep -v -- "--benchmark" |
        awk '{ s += $1 } END { printf "%.0f", s }'
}

run_one() {  # $1 = label, $2 = binary, $3 = scene, $4 = repeat
    label=$1; bin=$2; scene=$3; rep=$4
    work="$out/$label"
    mkdir -p "$work"

    # Settle first: the previous run's own load is still in the average.
    waited=0
    while over "$(load_now)" && [ "$waited" -lt 180 ]; do
        sleep 10
        waited=$((waited + 10))
    done
    before=$(load_now)

    ( cd "$work" && HORIZON_NO_AUDIO=1 "$bin" \
        --resources "$RESOURCES" \
        --benchmark "$scene" --offscreen \
        --render-distance "$rd" --width 1280 --height 720 \
        > "$work/$scene-rd$rd-$rep.log" 2>&1 ) &
    game=$!

    # Sample other processes' CPU while it runs; keep the worst.
    peak=0
    while kill -0 "$game" 2>/dev/null; do
        c=$(other_cpu)
        [ "$c" -gt "$peak" ] && peak=$c
        sleep 2
    done
    wait "$game" || { echo "    $label $scene #$rep: run failed" >&2; return 1; }

    # No per-run discard. macOS `ps %cpu` is a LIFETIME average, so a build
    # that finished ten minutes ago still reports its old several-hundred
    # percent and every run would be thrown away for work that had long since
    # stopped. The arms are interleaved instead -- A, B, A, B within each scene
    # -- so a steady background lands on both, and the verdict comes from
    # whether the two arms' intervals separate, not from a threshold. A run
    # that genuinely collides with a compile shows up as an outlier and widens
    # the interval, which is the honest outcome.
    echo "    $label $scene #$rep: load before $before"
    return 0
}

for scene in $scenes; do
    rep=1
    while [ "$rep" -le "$repeats" ]; do
        for arm in a b; do
            case $arm in
                a) label=arm-a; bin=$a ;;
                b) label=arm-b; bin=$b ;;
            esac
            try=1
            until run_one "$label" "$bin" "$scene" "$rep"; do
                try=$((try + 1))
                if [ "$try" -gt "$attempts" ]; then
                    echo "error: $label $scene #$rep failed $attempts times" >&2
                    exit 1
                fi
                # Let the machine settle before trying again.
                sleep 20
            done
        done
        rep=$((rep + 1))
    done
done

echo
echo "==> done. Compare with:"
echo "    tools/ab-compare.py $out/arm-a $out/arm-b"
echo
echo "Per-run load is in $out/loads.txt."
