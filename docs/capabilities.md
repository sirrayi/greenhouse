# What horizon-mixins can and cannot express

Measured, not assumed. Every row below is the result of running the real
`horizon-mixins` CLI (pinned revision `d9c83d7`, plan schema 6) against a pristine export of
Horizon at `ba6344a59` and reading the result.

## Environment

| Fact | Result |
| --- | --- |
| Builds on macOS arm64 | yes, on its pinned 1.95.0 toolchain (`cargo build --release --locked`) |
| `plan` on a pristine pinned checkout | yes, `status: ok` |
| `apply` on a pristine pinned checkout | yes, `status: ok`, stage written |

Staging gotchas, each of which cost a run:

1. `/tmp` is a symlink on macOS; the tool rejects output paths that traverse symlinks. Use
   `/private/tmp` or a path under `$HOME`.
2. The Horizon tree contains `CLAUDE.md -> AGENTS.md`, and staged inputs may not contain
   symlinks. Materialise it before planning.
3. A support module may not begin with `//!` inner doc comments — they are file attributes.
   Use `//`.
4. `cargo` must be on `PATH` for the child `cargo metadata` invocation.

## The four probes

Each probe is a minimal bundle targeting a real Horizon function.

| # | Target | Kind | Result |
| --- | --- | --- | --- |
| 1 | `gfx::page_runs::PageRuns::push` (body has no closures) | `OVERWRITE` | **ok** |
| 2 | `gfx::translucency::Sorter::sort` (body contains closures) | `OVERWRITE` | **`E_UNSUPPORTED_CONTROL_FLOW`** |
| 3 | `gfx::translucency::Sorter::sort` (same body) | `HEAD` | **ok** |
| 4 | `gfx::page_runs::PageRuns::regroup` — a `BEFORE_CALL` on `self.runs.sort_by_key` *inside* that body | `BEFORE_CALL` | **`E_UNSUPPORTED_CONTROL_FLOW`** |
| 5 | `gfx::translucency::quad_center`, helper returning `ControlFlow<[f32; 3]>` | `HEAD`, `cancellable = true` | **ok** |
| 6 | `gfx::translucency::Sorter::sort`, helper returning `ControlFlow<&'_ [u32]>` | `HEAD`, `cancellable = true` | **`E_INVALID_HELPER`** — "no explicit generic parameters" |

## The rules

Three restrictions decide everything:

1. **Closures poison a body.** `OVERWRITE` — and any call-level hook such as `BEFORE_CALL` —
   fails with `E_UNSUPPORTED_CONTROL_FLOW` if the target function's body contains a closure.
2. **`HEAD` is exempt.** Prepending code works regardless of what the body contains, so
   instrumentation and side effects are always available.
3. **Helpers cannot declare generics or lifetimes.** A cancelling `HEAD` helper must return
   `ControlFlow<R>`, where `R` is the target's return type; for a target returning a borrowed
   slice that type needs a lifetime, and a lifetime cannot be written. So whole-function
   substitution works only for targets that return **owned** values.

## What that means for this codebase

Horizon's renderer is idiomatic Rust: hot functions are full of `iter().map(..)`,
`windows(2).all(..)`, `sort_by_key(|v| ..)` and `array::from_fn(..)`. Combining rules 1 and 3:

| Wanted change | Target returns | Verdict |
| --- | --- | --- |
| Replace the translucent quad sort | `&[u32]` (borrowed) | **Not expressible.** Body has closures (rule 1) and the return is borrowed (rule 3). |
| Replace draw regrouping with a counting sort | `&[Run]` (borrowed) | **Not expressible.** Same two reasons. |
| Hoist per-column work in the visibility walk | `()` | Not expressible: body has closures (rule 1). `HEAD` can only add, not rewrite. |
| Instrument and count | anything | **Expressible** via `HEAD` |
| Rewrite a closure-free function | owned | **Expressible** via `OVERWRITE` |

So the mod can observe the renderer and add work to it, and it can rewrite the small number of
closure-free functions, but it cannot replace the three hot functions the performance analysis
pointed at. That is a limitation of the tool, not of the analysis: the replacement algorithms
are written and proven correct (see `runtime.rs`), and they will drop straight in when
`horizon-mixins` gains closure support, or if they are contributed to Horizon itself.
