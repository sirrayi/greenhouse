#!/bin/sh
# Apply greenhouse's patch series to a Horizon checkout.
#
#   tools/apply-patches.sh <horizon-checkout> [--check] [--only NAME]... [--except NAME]...
#
# This is the second delivery path, and the reason it exists: horizon-mixins
# refuses to rewrite a function whose body contains a closure, and cannot
# substitute a function that returns borrowed data. Horizon's hot renderer
# functions are both, so the draw-path changes cannot ship as a mixin. They ship
# as patches instead, still from this repository and still without touching
# upstream.
#
# Every change is individually switchable: --only selects changes by name
# fragment, --except drops them. Build one, build another, compare the two
# binaries with tools/ab.sh.
#
#   tools/apply-patches.sh ~/horizon --only regroup
#   tools/apply-patches.sh ~/horizon --except cull
#
# <horizon-checkout> must be at the revision in compatibility.json. --check
# tests that every selected patch applies without writing anything.
set -eu

horizon=${1:?usage: apply-patches.sh <horizon-checkout> [--check] [--only NAME]... [--except NAME]...}
shift
here=$(cd "$(dirname "$0")/.." && pwd)

mode=apply
only=""
except=""
while [ $# -gt 0 ]; do
    case $1 in
        --check) mode=check ;;
        --only) only="$only ${2:?--only needs a name}"; shift ;;
        --except) except="$except ${2:?--except needs a name}"; shift ;;
        *) echo "error: unknown argument: $1" >&2; exit 1 ;;
    esac
    shift
done

# Whether a patch file name matches a selector list. An empty list matches
# nothing, which is what --only with no names must mean.
matches() {
    names=$1
    name=$2
    for want in $names; do
        case $name in
            *"$want"*) return 0 ;;
        esac
    done
    return 1
}

revision=$(sed -n 's/.*"horizon_revision": "\([0-9a-f]*\)".*/\1/p' "$here/compatibility.json")
head=$(git -C "$horizon" rev-parse HEAD 2>/dev/null || true)

if [ "$head" != "$revision" ]; then
    cat >&2 <<EOF
error: $horizon is at ${head:-<not a git checkout>}, expected $revision
       (git -C $horizon checkout $revision)
EOF
    exit 1
fi

applied=0
skipped=0
for patch in "$here"/patches/*.patch; do
    [ -e "$patch" ] || continue
    name=$(basename "$patch")
    if [ -n "$only" ] && ! matches "$only" "$name"; then
        echo "  skipped    $name (not selected)"
        skipped=$((skipped + 1))
        continue
    fi
    if [ -n "$except" ] && matches "$except" "$name"; then
        echo "  skipped    $name (excluded)"
        skipped=$((skipped + 1))
        continue
    fi
    if [ "$mode" = check ]; then
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
    applied=$((applied + 1))
done

[ "$applied" -gt 0 ] || { echo "error: nothing selected from $here/patches" >&2; exit 1; }

if [ "$mode" != check ]; then
    cat <<EOF

Applied $applied patch(es) to $horizon ($skipped skipped).

Now build and test it:

  cd $horizon
  cargo build --release -p horizon-client --bins
  cargo test -p horizon-client-render --lib

Then measure it against an unpatched build with tools/ab.sh, on an idle
machine. A patch that is not measured against a baseline is not a change you
can claim.
EOF
fi
