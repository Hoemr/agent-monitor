# Character pet validation

- Rust: `cargo test --manifest-path src-tauri/Cargo.toml` — 265 passed, 0 failed. Includes all new pack states, picker discovery, activation and legacy settings migration.
- Settings browser suite: 111 checks passed, including each new thumbnail loading and selection saving. Full output: `settings-selftest.txt`.
- Desktop-pet browser suite: ALL PASS. The shell wrapper encountered a Chrome teardown hang; the settings suite was rerun with an isolated profile and file-backed output and passed.
- Assets: 24 APNGs, 48 decoded 512×512 frames; nonempty artwork and transparent 16-pixel safety margins verified.
- Visual QA: light/dark backgrounds and 48/72/96-pixel desktop sizes inspected; settings picker screenshot inspected.
- Release build: completed successfully with the repository's existing release profile.

Generated source sheets and exact built-in image_gen prompts: `PROMPTS.md`. Runtime assets: `../../src-tauri/pets/{nailong,doraemon,tihun}/`.

Local install: app bundled and signature verified; original designated requirement preserved, application restarted successfully, settings unchanged and all three existing cat packs retained. Previous bundle: `../../dist/Agent Monitor.before-character-pets.app`.
