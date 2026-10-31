// Runs at the head of every frame the renderer draws.
//
// The counters live in the support module injected beside `PageRuns` in the
// same file tree, so this reaches them directly and cheaply: one relaxed
// increment per frame, and a log line every 600.
// Not `frame`: the target's own parameter is called that, and a helper may not
// shadow a target parameter.
fn report() {
    page_runs::greenhouse::report_frame();
}
