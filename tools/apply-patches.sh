#!/bin/sh
# Apply greenhouse's patch series to a Horizon checkout.
#
#   tools/apply-patches.sh <horizon-checkout> [--check]
#
# This is the second delivery path, and the reason it exists: horizon-mixins
# refuses to rewrite a function whose body contains a closure, and cannot
# substitute a function that returns borrowed data. Horizon's hot renderer
# functions are both, so the draw-path changes cannot ship as a mixin. They ship
# as patches instead, still from this repository and still without touching
# upstream.
#
# <horizon-checkout> must be at the revision in compatibility.json. --check
# tests that every patch applies without writing anything.
set -eu

horizon=${1:?usage: apply-patches.sh <horizon-checkout> [--check]}
here=$(cd "$(dirname "$0")/.." && pwd)
mode=${2:-apply}

revision=$(sed -n 's/.*"horizon_revision": "\([0-9a-f]*\)".*/\1/p' "$here/compatibility.json")
head=$(git -C "$horizon" rev-parse HEAD 2>/dev/null || true)

if [ "$head" != "$revision" ]; then
    cat >&2 <<EOF
error: $horizon is at ${head:-<not a git checkout>}, expected $revision
       (git -C $horizon checkout $revision)
EOF
    exit 1
fi

found=0
for patch in "$here"/patches/*.patch; do
    [ -e "$patch" ] || continue
    found=$((found + 1))
    name=$(basename "$patch")
    if [ "$mode" = "--check" ]; then
        if git -C "$horizon" apply --check "$patch" 2>/dev/null; then
            echo "  applies    $name"
        else
            echo "  CONFLICT   $name" >&2
            exit 1
        fi
    else
        if git -C "$horizon" apply "$patch"; then
            echo "  applied    $name"
        else
            echo "  FAILED     $name" >&2
            exit 1
        fi
    fi
done

[ "$found" -gt 0 ] || { echo "error: no patches in $here/patches" >&2; exit 1; }

if [ "$mode" != "--check" ]; then
    cat <<EOF

Applied $found patch(es) to $horizon.

Now build and test it:

  cd $horizon
  cargo build --release -p horizon-client --bins
  cargo test -p horizon-client-render --lib

Then measure it against an unpatched build with tools/ab.sh, on an idle
machine. A patch that is not measured against a baseline is not a change you
can claim.
EOF
fi
