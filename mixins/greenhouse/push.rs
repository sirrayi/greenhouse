// Replaces `PageRuns::push` in crates/horizon-client-render/src/gfx/page_runs.rs.
//
// Behaviour-identical: the band arithmetic is factored into the support module
// and called, and a counter records the draw volume; nothing else changes. This
// is the scaffold the rest of the draw-path hooks hang off, and it keeps the
// bundle planning and compiling while the larger changes are still blocked
// (see README: an OVERWRITE cannot rewrite a body that contains closures).
//
// Compiled inside `push`, so `Run` and its private `key` field are in scope.
fn push(
    runs: &mut PageRuns,
    distance: u64,
    page: u32,
    start: usize,
    len: usize,
    tag: [u32; 3],
) {
    if len > 0 {
        let band = greenhouse::band_of(BAND, distance);
        greenhouse::count_push();
        runs.runs.push(Run {
            key: band << 32 | u64::from(page),
            start: start as u32,
            len: len as u32,
            tag,
        });
    }
}
