# Interpolation-Engine Rust Rewrite: Current Status (Updated 2026-02-06)

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
- `cargo build` last run in this session (clean).

Local LLM server:
- API default changed to `http://0.0.0.0:8080` (still overrideable via `api_url`).

Verified runs (agent-mode) in this session:
- `examples/goto.json5` runs.
- `examples/text_adventure.json5` blocked here due to sandbox network restrictions on outbound TCP (model server not reachable in this environment).

## Approving Network Access (Agent Mode)
This environment blocks outbound TCP in the sandbox. To let agent-mode reach a model server, run the binary outside the sandbox and then provide agent input.

Steps:
1. Run with escalation:
   - `./target/debug/interpolation-engine --agent-mode ../examples/text_adventure.json5`
2. Feed initial input:
   - `printf "A castle at dusk.\n" > /tmp/agent_input`
3. Check logs:
   - `tail -n 80 /tmp/agent_run.log`

Expected if the server is reachable but the model name is invalid:
`Error: Chat request failed: 400 Bad Request {"error":{"code":400,"message":"model not found","type":"invalid_request_error"}}`

Note: accessing `http://0.0.0.0:8080` requires elevated permissions here; if a test needs localhost, run with escalation.

## Known Issues / Gaps
- Chat examples still not validated in this environment: local socket access for chat is blocked in agent-mode runs (see latest run attempt).
- Output scrolling in the TUI was missing; fixed to support PageUp/PageDown, Ctrl+Up/Down, Home/End, and mouse wheel scrolling (auto-scroll keeps the view pinned to the bottom unless the user scrolls up).

## Near-Term Plan
1. **Finish compatibility testing**:
   - Run `api.json5`, `character_creator.json5`, `text_adventure*.json5` once local socket access is available.
2. **Polish warnings**:
   - Keep builds warning-free.
3. **Stabilize error handling**:
   - Ensure all runtime errors preserve terminal state (already fixed for main TUI shutdown).
4. **Static analyzer improvements**:
   - Expand interpolation key analysis (closer to Python behavior).
   - Add better diagnostics for `goto_map` and nested tasks.

## Recent Improvements
- TUI: user_choice options now render fully; choice text is bottom-aligned like user_input.
- Main menu: Esc open/close stable, save/load/restart keeps menu open to display status messages.
- Input history: `--history` now records inputs; Up/Down navigation and Ctrl-R reverse search supported (multi-line entries preserved).
- Input editing: cursor-aware line editing with Ctrl-A/E, Ctrl-W, Ctrl-Left/Right, Home/End/Delete, and mid-line insert.
- Input latency: UI redraws only on change (dirty flag), reducing typing lag.
- Output scrolling: added scroll offset with auto-scroll to bottom, PageUp/PageDown + mouse wheel support, and Ctrl+Up/Down for line scrolling.
- Analyzer: strengthened definite-error checks (per-scope labels, goto_map literal target resolution, list bounds/length validation, and stricter type checks) without speculative warnings.
- list_slice: supports `to_index == 0` and returns empty list when `to_index < from_index`.
- random_choice: now errors on empty lists (matches Python behavior).
- chat: parses `n_outputs`/`shown` string values, retries if fewer outputs than requested, strips `line`/`traceback_label` before API call, treats empty `voice_path` as disabled.
- Validation: `voice_path` is checked for literal paths during program analysis and at runtime before starting TTS.
- Analyzer: added type checks for common fields (strings, arrays, task arrays), simple-interpolation type resolution using default inserts, and stricter `goto_map`/`replace_map` shape validation.
- Logging: `--log` writes human-readable text (task previews, chat transcripts, for-loop iteration detail).
- Fix: `user_choice` with an empty list now blocks and can be cancelled, so `parallel_race` no longer cancels generation immediately (e.g. `examples/text_adventure.json5`).
- Fix: `parallel_race` now clears `order_index/{runtime_label}` for interrupted tasks (like Python), preventing serial tasks from resuming mid-way and skipping chat on the next loop.
- Fix: `chat` now merges `extra_body` into the top-level request (grammar enforcement works like Python), avoiding missing `<output>` tags and zero-output retries.
- Audio web: optional `--audio-web`/`--audio-port` serves a minimal page with streaming WAV audio; keepalive silence + reconnection; delays shutdown to finish playback.

## Commands I Use
- Build: `cargo build` (from `rust-project/`)
- Run (agent mode): `./target/debug/interpolation-engine --agent-mode ../examples/hello_world.json5`

## Workflow Note
Always build your changes before asking for feedback.
