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
        Vec::new()
    }
}
```

Rules worth re-reading in `src-tauri/src/model.rs`:

* `TaskStatus::WaitingInput` / `WaitingApproval` require an explicit signal.
  Silence, age, or “no output” must map to `Idle` / `Unknown` — never waiting.
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
- [ ] No new dependency, no background process, no per-task timer.
- [ ] `docs/CAPABILITIES.md` row added with the verified level and limitations.
