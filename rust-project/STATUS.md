# Interpolation-Engine Rust Rewrite: Current Understanding and Plan

## Understanding (Behavior + Compatibility)
Interpolation-engine is a CLI runtime that executes "programs" defined in JSON5. A program has:
- `default_state`: includes `order_index` and `inserts` (stateful key/value storage).
- `order`: list of tasks (commands) executed sequentially.
- `named_tasks`: map of named tasks (invoked via `run_task`).
- `save_states`: map of save slots stored in the program JSON5 itself.
- Optional `completion_args` for `chat`.

Key runtime behaviors:
- **Interpolation**: Every string can interpolate `{key}` from `state.inserts`, with support for escaped braces (`\{`, `\}`), nested inserts, and fallback to `--inserts-dir` files.
- **Order execution**: 1-based `order_index` with `goto`/`goto_map` control flow.
- **Parallel tasks**: `parallel_wait` (wait all), `parallel_race` (first wins, cancel others).
- **Main menu**: toggled with `Esc`, pauses execution, supports save/load/restart/quit.
- **Agent mode**: file-based control using `/tmp/agent_output` (JSON prompt) + `/tmp/agent_input` (selected input) so agents can drive interactive programs.

Compatibility targets to preserve:
- All existing examples in `examples/*.json5`.
- Main menu behavior (pause/resume, save/load).
- Line numbers on tasks for useful error messages.

Non-goals:
- Save format compatibility with Python version.
- Specific usage of `runtime_label`/`traceback_label` (but still include useful line/context in errors).

## Rust Rewrite: Current State
Implemented in `rust-project/` with the following:
- Parser with line-number injection: `src/parser.rs`
- Static analyzer: `src/analyzer.rs`
- Interpreter/runtime: `src/runtime.rs`
- Interpolation + escape/unescape: `src/interp.rs`
- Math expression evaluation: `src/math.rs`
- TUI: `src/ui.rs` (full screen, instant keypress for `user_choice`, `Esc` menu)
- Chat (OpenAI compatible SSE): `src/chat.rs`
- Save-state splicing into JSON5: `src/save.rs`

Build:
- `reqwest` uses `rustls-tls` (no OpenSSL).
- `cargo build` succeeds.

Verified runs (agent-mode):
- `examples/hello_world.json5`
- `examples/math.json5`
- `examples/interactivity.json5`

Blocked in this environment:
- Chat-based examples (local socket access to `localhost:8080` is blocked here).

## Known Issues / Gaps
- Many warnings (unused imports/vars) need cleanup.
- Static analyzer is simpler than Python version; it catches missing fields/labels but is not as exhaustive.
- Chat examples cannot be fully validated in this sandbox due to local socket restrictions.

## Near-Term Plan
1. **Finish compatibility testing**:
   - Run `api.json5`, `character_creator.json5`, `text_adventure*.json5` once local socket access is available.
2. **Polish warnings**:
   - Remove unused imports/vars.
   - Trim dead fields (e.g., unused `log_path`/`history_path`) or implement them.
3. **Stabilize error handling**:
   - Ensure all runtime errors preserve terminal state (already fixed for main TUI shutdown).
4. **Static analyzer improvements**:
   - Expand interpolation key analysis (closer to Python behavior).
   - Add better diagnostics for `goto_map` and nested tasks.

## Commands I Use
- Build: `cargo build` (from `rust-project/`)
- Run (agent mode): `./target/debug/interpolation-engine --agent-mode ../examples/hello_world.json5`

