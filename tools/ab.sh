#!/bin/sh
# A/B two Horizon builds on the same world and camera path, and refuse to
# report when the machine is too busy for the numbers to mean anything.
#
#   tools/ab.sh <label> <horizon-client binary> [scene ...]
#
# Scenes default to "village ocean forest". Render distance is fixed at 32 by
# default because that is the only distance where these changes are measurable:
# at RD 8 a frame is 0.26 ms and the 3-repeat confidence interval is +/-7.9%,
# wider than any effect this mod can produce, on a frame rate 29x a 120 Hz
# panel. See the README.
#
# Environment:
#   RESOURCES   prepared 26.3 resources (horizon-prepare output), required
#   SCENE_RD    render distance, default 32
#   REPEATS     runs per scene, default 3 (raise to 7+ to resolve 5%)
#   MAX_LOAD    refuse to start above this 1-minute load average, default 1.0
#               (per core: the script compares against the core count)
#   OUT         report directory, default ./ab-reports
set -eu

label=${1:?usage: ab.sh <label> <horizon-client binary> [scene ...]}
bin=${2:?usage: ab.sh <label> <horizon-client binary> [scene ...]}
shift 2
scenes=${*:-village ocean forest}

[ -n "${RESOURCES:-}" ] || { echo "error: set RESOURCES to prepared 26.3 resources" >&2; exit 1; }
[ -x "$bin" ] || { echo "error: not executable: $bin" >&2; exit 1; }

rd=${SCENE_RD:-32}
repeats=${REPEATS:-3}
out=${OUT:-./ab-reports}
mkdir -p "$out"

# --- the gate -----------------------------------------------------------------
# A benchmark taken next to a build is not a benchmark. Horizon's own published
# runs discard any game during which another process used more than 15% of the
# machine for 10 seconds; this is the cheap version of that rule.
cores=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)
budget=$(awk -v c="$cores" -v m="${MAX_LOAD:-1.0}" 'BEGIN { printf "%.2f", c * m }')
load=$(uptime | sed 's/.*load averages*: *//' | awk '{print $1}' | tr -d ',')
if awk -v l="$load" -v b="$budget" 'BEGIN { exit !(l > b) }'; then
    cat >&2 <<EOF
refusing to measure: 1-minute load average is $load on $cores cores
(budget $budget). Numbers taken now would be noise. Wait, or raise MAX_LOAD
deliberately and label the result as taken under load.

Horizon's own benchmark documentation discards runs taken on a busy machine for
exactly this reason; this script does the same automatically.
EOF
    exit 2
fi
echo "==> load $load <= budget $budget on $cores cores"

# --- the runs -----------------------------------------------------------------
total=0
for scene in $scenes; do
    for i in $(seq 1 "$repeats"); do
        total=$((total + 1))
    done
done
echo "==> $label: $total runs ($repeats x $scenes at RD $rd, offscreen)"
echo "    binary: $bin"

for scene in $scenes; do
    i=1
    while [ "$i" -le "$repeats" ]; do
        # --offscreen renders into a texture and never shows or focuses a window,
        # so a run cannot be disturbed by whatever else is on the desktop.
        "$bin" \
            --resources "$RESOURCES" \
            --benchmark "$scene" \
            --offscreen \
            --render-distance "$rd" \
            --width 1280 --height 720 \
            > "$out/$label-$scene-rd$rd-$i.log" 2>&1 || {
                echo "error: run failed, see $out/$label-$scene-rd$rd-$i.log" >&2
                exit 1
            }
        echo "    $scene repeat $i done"
        i=$((i + 1))
    done
done

echo "==> reports written under $out"
echo "    compare two labels with: tools/ab-compare.py $out/<label> $out/<other>"
