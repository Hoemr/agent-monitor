# Agent Monitor for Windows

Windows 10/11, x64. Use the NSIS setup program for a normal per-user install;
administrator rights are not required. The installer installs WebView2 if it is
missing (internet access required for that download).

The portable ZIP contains `agent-monitor.exe`; extract it and run it. Portable
mode still stores preferences in `%LOCALAPPDATA%\agent-monitor`. It needs an
existing Microsoft Edge WebView2 Runtime. Windows notification delivery is
intended for the installed app; prefer the installer if notifications matter.

## Monitoring

Native Windows sessions are read from the current user's profile:

| Agent | Default local source |
|---|---|
| Codex CLI / VSCode | `%USERPROFILE%\.codex\state_5.sqlite` and recorded rollout JSONL |
| Claude Code CLI / VSCode | `%USERPROFILE%\.claude\projects\*\*.jsonl` |
| DSH | `%USERPROFILE%\.dsh\storages\session_projcache\sessions` |
| ZCode | `%USERPROFILE%\.zcode\v2\tasks-index.sqlite` |
| Antigravity | `%USERPROFILE%\.gemini\antigravity` and `antigravity-cli` |

The first Windows release targets native Windows terminals and VSCode.
It does not automatically discover WSL profiles. A custom Codex home list can
be configured in `%LOCALAPPDATA%\agent-monitor\codex-homes.json` using absolute
Windows paths (escape backslashes in JSON, or use forward slashes).

## Features and limits

- Desktop pet artwork and animations, tray controls, edge snapping/hiding,
  settings, pinned/custom-named conversations and unread completion markers.
- Windows process detection uses the native process snapshot API.
- Clicking a task focuses a running terminal, VSCode or agent application at
  L1. It does not verify a conversation or terminal tab. With multiple terminal
  windows, select the intended window/tab yourself. Keep the host application
  open. Windows can refuse foreground focus, especially across elevation levels.
- Mark individual or all completed conversations as read in the panel.
  Focusing a Windows application does not acknowledge its conversations.
- No macOS Accessibility or Automation permissions are needed. The Windows
  settings page explains Windows focus behavior instead.
- This build uses manual downloads for updates. It is not Authenticode signed;
  Windows may show an unknown-publisher/SmartScreen prompt.

## Building

Install Rust's MSVC toolchain, Visual Studio C++ Build Tools, Node.js and WebView2.
From the repository root:

```powershell
cargo test --release --locked --manifest-path src-tauri/Cargo.toml
npx --yes @tauri-apps/cli@2.11.4 build --bundles nsis
```

The `Build Windows` GitHub Actions workflow runs Rust and UI tests, creates the
installer/portable ZIP, verifies a native snapshot and checks that the app stays
running on the Windows runner. Human verification on a normal desktop is still
needed for drag/drop, mixed-DPI monitors, taskbar/tray interactions and real
agent application switching.
