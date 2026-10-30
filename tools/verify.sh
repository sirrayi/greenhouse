#!/bin/sh
# Plan, apply and check greenhouse against a pristine Horizon export.
#
# horizon-mixins never edits the source checkout: it exports the pinned revision
# with `git archive` into a staging directory and transforms that.
#
#   HORIZON=/path/to/horizon  MIXINS=/path/to/horizon-mixins  tools/verify.sh
#
# Both checkouts must contain the revisions in compatibility.json.
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
HORIZON=${HORIZON:?set HORIZON to a horizon checkout containing the pinned revision}
MIXINS=${MIXINS:?set MIXINS to a horizon-mixins checkout}

revision=$(sed -n 's/.*"horizon_revision": "\([0-9a-f]*\)".*/\1/p' "$here/compatibility.json")
mixins_revision=$(sed -n 's/.*"horizon_mixins_revision": "\([0-9a-f]*\)".*/\1/p' "$here/compatibility.json")

echo "==> horizon revision $revision"
git -C "$HORIZON" cat-file -e "$revision^{commit}" 2>/dev/null || {
    echo "error: $HORIZON does not contain $revision (try: git -C $HORIZON fetch --tags)" >&2
    exit 1
}
git -C "$MIXINS" cat-file -e "$mixins_revision^{commit}" 2>/dev/null || {
    echo "error: $MIXINS does not contain $mixins_revision" >&2
    exit 1
}

echo "==> building horizon-mixins at $mixins_revision"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git -C "$MIXINS" archive "$mixins_revision" | tar -x -C "$work"
cargo build --release --locked --offline --manifest-path "$work/Cargo.toml" -p horizon-mixins-cli
cli="$work/target/release/horizon-mixins"

echo "==> plan"
"$cli" plan --workspace "$HORIZON" --mods "$here/mixins" --out "$work/plan.json"
echo "    $(wc -c < "$work/plan.json") bytes of plan"

echo "==> apply"
"$cli" apply --workspace "$HORIZON" --mods "$here/mixins" --out "$work/stage"

echo "==> check (compiles the staged game)"
"$cli" check --workspace "$HORIZON" --mods "$here/mixins" --out "$work/checked" \
    --package horizon-client-render --trust-native-mods

echo "==> ok: greenhouse plans, applies and compiles against $revision"
