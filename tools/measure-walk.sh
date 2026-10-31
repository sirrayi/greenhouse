#!/bin/sh
# Measure Resident::visible on Horizon's own benchmark, pristine and patched.
#
#   HORIZON=/path/to/horizon tools/measure-walk.sh [profile]
#
# `profile` defaults to `fast` (opt-level 3, no LTO). It matters: the same
# comparison in a debug build showed a 52% win, and a debug build cannot inline
# or hoist, so it flatters any change that removes redundancy the optimizer
# would have removed for free. Only an optimized profile answers the real
# question.
#
# The benchmark is Horizon's own `resident::tests::column_walk_speed`, ignored
# by default. It asserts that the column walk finds exactly the sections the
# reference walk does, and reports the best of 200 runs -- a minimum, which is
# why a loaded machine can still give a usable number: a sample that was
# descheduled is only ever slower.
#
# The checkout is left exactly as it was found.
set -eu

horizon=${HORIZON:?set HORIZON to a Horizon checkout at the pinned revision}
profile=${1:-fast}
here=$(cd "$(dirname "$0")/.." && pwd)

revision=$(sed -n 's/.*"horizon_revision": "\([0-9a-f]*\)".*/\1/p' "$here/compatibility.json")
head=$(git -C "$horizon" rev-parse HEAD 2>/dev/null || true)
if [ "$head" != "$revision" ]; then
    echo "error: $horizon is at $head, expected $revision" >&2
    exit 1
fi
if [ -n "$(git -C "$horizon" status --porcelain)" ]; then
    echo "error: $horizon has local changes; refusing to patch it" >&2
    exit 1
fi

run() {
    # shellcheck disable=SC2086
    cd "$horizon"
    CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$horizon/target} \
        cargo test --profile "$profile" -p horizon-client-render --lib -- \
        --ignored column_walk_speed --nocapture 2>&1 |
        grep -E 'resident, [0-9]+ visible' || true
}

echo "==> pristine"
pristine=$(run)
echo "    $pristine"

echo "==> patched"
"$here"/tools/apply-patches.sh "$horizon" >/dev/null
patched=$(run)
git -C "$horizon" checkout -- . || true
echo "    $patched"

echo
echo "==> comparison ($profile)"
echo "    pristine: $pristine"
echo "    patched:  $patched"
echo
echo "Read the 'column walk' figure, not 'old walk' (that one is the"
echo "pre-column-walk reference and does not change). The checksum and the"
echo "visible count must match between the two runs; the benchmark asserts the"
echo "count, and a differing checksum would mean the walk changed what it found."
