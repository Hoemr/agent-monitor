# Adding a new Agent adapter

The monitor and the UI know nothing about specific agents. Adding an agent means
adding **one adapter file** and one line in the registry. No UI change, no
monitor change, no new window, no new timer.

## 1. Implement the `Source` trait

Create `src-tauri/src/agents/my_agent.rs`:

```rust
use super::{truncate_chars, ScanCtx, Source};
use crate::model::*;

pub struct MyAgentSource { /* incremental caches (mtime, byte offsets) */ }

impl MyAgentSource { pub fn new() -> Self { Self { /* … */ } } }

impl Source for MyAgentSource {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            key: "my-agent".into(),
            name: "My Agent".into(),
            source: "~/.my-agent/sessions/*.json".into(),
            origin: DataOrigin::Event,      // Event | Index | Cache | Synthetic
            max_level: JumpLevel::L2,       // never higher than verified
            notes: "window-level jump by workspace".into(),
        }
    }

    fn scan(&mut self, ctx: &ScanCtx) -> Vec<TaskRecord> {
        // 1. read only what is needed, cheaply (mtime/size checks first)
        // 2. one TaskRecord per session — never one record per agent
        // 3. stable id `my-agent:<source-session-id>` (never the title)
        // 4. status only from explicit evidence; put the evidence in status_reason
        // 5. last_activity_secs from the source timestamp (not ctx.now_secs)
        // 6. fill JumpTarget (app/bundle/window_title/workspace/deep_link)
        // 7. target_validity + capability + refresh_level()
        // 8. honor ctx.pinned_ids before filtering by age
        // 9. model = latest actual parent-turn model, or None if unavailable
        Vec::new()
    }
}
```

Rules worth re-reading in `src-tauri/src/model.rs`:

* `TaskStatus::WaitingInput` / `WaitingApproval` require an explicit signal.
  Silence, age, or “no output” must map to `Idle` / `Unknown` — never waiting.
  Track unanswered interactive calls by call id (and question index for async
  prompts). Acceptance, unrelated results and subagent completion cannot settle
  a parent question. Clear it only on an explicit answer/cancellation or the end
  of that parent turn.
* `TaskStatus::Completed` requires explicit final completion of the current
  **parent task or turn**, with `TaskRecord.completion = Some(CompletionEvidence::…)`.
  Choose `TaskEnd`, `ClosedTurn`, `AssistantEndTurn`, or `TaskIndex` only when the
  source actually provides that evidence. Tool results, step/item completion,
  subagent results, ordinary assistant text, and silence are never evidence of
  final completion. Keep an open task running, or report `Unknown` when its state
  cannot be established. A new task/turn invalidates the previous completion.
  Monitor applies `enforce_completion()` to every adapter, including future ones:
  a completion with no evidence becomes `Unknown` before reaching the UI. This
  internal evidence field is not part of the frontend JSON.
* `last_activity_secs` and `collected_at_secs` are different things; the UI shows
  the former.
* If the host can only be activated, declare `JumpLevel::L1` and set the notes
  that explain why; `jump.rs` will degrade with feedback instead of lying.
* Missing optional fields are fine — they lower the declared level.

## 2. Register it

`src-tauri/src/agents/mod.rs`:

```rust
pub mod my_agent;

pub fn build_registry() -> Vec<Box<dyn Source>> {
    vec![
        Box::new(claude_code::ClaudeCodeSource::new()),
        Box::new(codex::CodexSource::new()),
        Box::new(dsh::DshSource::new()),
        Box::new(zcode::ZCodeSource::new()),
        Box::new(my_agent::MyAgentSource::new()),   // ← one line
    ]
}
```

## 3. Unit-test the risky parts

At minimum: status mapping from recorded event literals, stable-id derivation,
and that two sessions of the same agent produce two records. Use synthetic
fixtures — never copy real user titles or paths into tests.
Completion tests must cover intermediate step/tool/subagent results, a previous
completed turn followed by a new open turn, missing/partial source records,
explicit final completion, and long silence without a final completion signal.

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

## 4. Verify against real data

```bash
cargo build --release --manifest-path src-tauri/Cargo.toml
src-tauri/target/release/agent-monitor --dump-agents   # descriptor + declared level
src-tauri/target/release/agent-monitor --dump-tasks    # real records, ids, levels
```

`--dump-tasks` also prints `dedupe: … stable=true` after two consecutive scans —
both must match.

## 5. Optional: prove the UI path with the fixture adapter

The built-in `simulated` adapter is the acceptance fixture for extension work:

```bash
AGENT_MONITOR_SIMULATED=1 src-tauri/target/release/agent-monitor --dump-tasks | jq '.tasks[] | select(.origin=="synthetic")'
```

It renders with a `SIM` marker and `origin: "synthetic"`; it is never enabled
without the environment variable and never mixed silently with real records.
If a new agent shows up correctly through the snapshot, no UI change was needed.

## Checklist before merging

- [ ] `cargo test` passes; new mappings have tests.
- [ ] `--dump-agents` declares a level you actually verified (see `docs/JUMP.md`).
- [ ] `--dump-tasks` shows real records; two scans give identical ids (`stable=true`).
- [ ] Two parallel sessions of the new agent appear as two task bars.
- [ ] Status reasons cite evidence; no waiting state inferred from silence.
- [ ] Only final parent-task/turn completion supplies `CompletionEvidence`; intermediate work never does.
- [ ] No new dependency, no background process, no per-task timer.
- [ ] `docs/CAPABILITIES.md` row added with the verified level and limitations.

Completion notifications require a stable `TaskRecord.completion_id` from the final answer or turn. Never derive it from file mtime, collection time, title, or generic activity. Sources without answer-level identity must not fabricate new answer versions.
