<div align="center">

# xm-relay

**Cross-agent session handoff & preflight verification for AI coding agents.**  
*Never lose work. Never silently lose context. Always verify before continuing.*

[![CI](https://github.com/iqbalmu/xm-relay/actions/workflows/ci.yml/badge.svg)](https://github.com/iqbalmu/xm-relay/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](rust-toolchain.toml)

[**Quick Install**](#-installation) • [**Quick Start**](#-quick-start) • [**CLI Reference**](#-cli-reference) • [**Workspace Crates**](#-workspace-crates)

</div>

---

## Why `xm-relay`?

When you hit a usage limit or switch models across AI coding agents (**OpenCode**, **Antigravity CLI**, **Oh My Pi**), copy-pasting the last few messages is unreliable. Agent summaries can hallucinate completed work, miss failed attempts, or contradict the actual state of your Git working tree.

`xm-relay` is a **local-first, strictly read-only** CLI tool that bridges coding agents safely:

1. **Discovers & Normalizes Sessions** across `opencode`, `antigravity-cli`, and `omp` without ever modifying source session databases or files.
2. **Inspects Live Git State** (branch, HEAD, staged/unstaged/untracked files, binary-safe diffs, and `AGENTS.md`/`CLAUDE.md` instructions) with `GIT_OPTIONAL_LOCKS=0` so `.git/index` is never mutated.
3. **Verifies Claims Against Disk:** Checks whether files reported as created/modified in a session actually exist in the workspace. Missing files or branch/HEAD drift trigger critical conflicts that **block continuation**.
4. **Redacts Secrets Automatically:** Scans transcripts, diffs, and project instructions for credentials (AWS, GitHub, Slack, Google, Stripe, `sk-` API keys, JWTs, private keys, URL credentials) and replaces them with `[REDACTED:<kind>]` before writing anything to disk.
5. **Enforces Target-Agent Preflight:** Generates a structured handoff package (`manifest.json`, `context.md`, `task-state.json`, `evidence.json`, `checksums.txt`) and a preflight prompt that requires the receiving agent to verify the workspace before writing code.

---

## 📦 Installation

### Option 1: One-Liner Script (Linux & macOS)
Downloads the latest prebuilt binary from GitHub Releases, verifies its SHA-256 checksum, and installs it to `~/.local/bin/xm-relay` (automatically falls back to `cargo install` if no prebuilt release is published yet):

```bash
curl -fsSL https://raw.githubusercontent.com/iqbalmu/xm-relay/main/install.sh | sh
```

<details>
<summary>Updating or customizing install directory</summary>

```bash
# Self-update to the latest release in-place:
xm-relay update

# Or customize the target directory during install:
curl -fsSL https://raw.githubusercontent.com/iqbalmu/xm-relay/main/install.sh | XM_RELAY_INSTALL_DIR=/usr/local/bin sh
```
</details>

### Option 2: `cargo install` (From GitHub)
```bash
cargo install --git https://github.com/iqbalmu/xm-relay.git --locked xm-relay
```

### Option 3: `cargo-binstall`
```bash
cargo binstall --git https://github.com/iqbalmu/xm-relay xm-relay
```

### Option 4: Build from Source
```bash
git clone https://github.com/iqbalmu/xm-relay.git
cd xm-relay
cargo install --path crates/cli
```

---

## 🚀 Quick Start

### Interactive Arrow-Key Mode (Recommended)
Simply run `xm-relay` with no arguments inside your project directory:

```bash
xm-relay
```

Use **`↑`/`↓` (or `j`/`k`) + `Enter`** to navigate the streamlined interactive flow:
1. **Main Menu:** Choose **Pindahkan Sesi / Buat Handoff** or **Riwayat Handoff Sebelumnya**.
2. **Pick Session:** Lists sessions in your active directory with human-friendly titles & relative timestamps. Use **`g`** to toggle Global scope, **`a`** to filter by agent, or **`/`** to search inline.
3. **Pick Target Agent:** Select `opencode`, `antigravity-cli` (`agy`), or `omp`.
4. **Immediate Action:** Copy the continuation prompt directly to your clipboard, print it to stdout, or launch the target agent CLI in the project directory with the prompt pre-loaded.

### Direct CLI Mode (For Scripting & Power Users)

```bash
# 1. Check detected agents and current repository status
xm-relay status

# 2. Find your latest session ID in the active directory (add -g for global)
xm-relay sessions list --agent opencode --limit 5

# 3. Build a verified, secret-redacted handoff package
xm-relay handoff create --from opencode --session <SESSION_ID> --to omp

# 4. Generate the preflight continuation prompt for the target agent
xm-relay continue <HANDOFF_ID>
```

> **Tip:** Pipe `xm-relay continue <HANDOFF_ID>` directly to your clipboard (`| wl-copy`, `| xclip -sel clip`, or `| pbcopy`) and paste it into your target coding agent. If a critical conflict is detected, `xm-relay continue` exits with code `2` and refuses to emit a prompt until the conflict is resolved.

---

## 🛠️ CLI Reference

Every subcommand supports `--help` and most provide `--json` for scripting.

| Command | Description |
|---|---|
| `xm-relay update` | Self-update `xm-relay` in-place to the latest GitHub release |
| `xm-relay agents` | Detect installed coding agents and their session data paths |
| `xm-relay status [path]` | Summary of detected agents, live Git state, checkpoints, and handoffs |
| `xm-relay sessions list` | List sessions for the active directory (`-g`/`--global` for all dirs, `--agent`, `--limit`) |
| `xm-relay sessions search <query>` | Search sessions in the active directory (`-g`/`--global` for all dirs) |
| `xm-relay sessions inspect <id>` | Inspect normalized events and transparency `parse_report` |
| `xm-relay project inspect [path]` | Read-only Git status, diff stats, untracked inventory, and recent commits |
| `xm-relay project instructions [path]` | Discover `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `.cursorrules`, Copilot rules |
| `xm-relay project scan [path]` | Heuristic secret scan across diffs, untracked files, and instructions (exits `2` on hit) |
| `xm-relay checkpoint create [path]` | Snapshot Git status, instructions, and redacted patch with SHA-256 checksums |
| `xm-relay checkpoint list \| inspect \| diff` | List checkpoints, verify artifact checksums, or compare live workspace drift |
| `xm-relay handoff create` | Build a portable handoff package (`--from`, `--session`, `--to`, `--project`) |
| `xm-relay handoff list \| inspect \| validate` | View handoff history, inspect `context.md`, or re-validate against live disk |
| `xm-relay handoff export <id> <dest>` | Copy a checksum-verified handoff package to an external directory |
| `xm-relay continue <id>` | Re-validate and emit the target-agent preflight continuation prompt |

---

## 📂 Portable Handoff Package Layout

Packages are stored in `~/.local/share/xm-relay/handoffs/<handoff-id>/` (or `$XDG_DATA_HOME/xm-relay/handoffs/`) and have zero dependency on any internal database:

```text
<handoff-id>/
├── manifest.json              # Metadata, Git summary, qualitative confidence, conflicts, file digests
├── context.md                 # Structured summary: Objective, Completed, Pending, Failed, Conflicts
├── task-state.json            # Extracted tasks with status, confidence, and evidence_refs
├── evidence.json              # Provenance & verification results (verified / reported / conflict)
├── session-normalized.json    # Secret-redacted canonical session events and parse report
├── repository-state.json      # Live Git status, diff summary, untracked file hashes, commits
├── project-instructions/      # Redacted copies of discovered AGENTS.md, CLAUDE.md, etc.
├── patches/
│   └── working-tree.patch     # Redacted tracked diff against HEAD
├── validation-report.json     # 7-point preflight checklist and status (Passed / Blocked / Failed)
└── checksums.txt              # SHA-256 digests of every package artifact + manifest.json
```

---

## 🔒 Security, Privacy & Known Limitations

- **Strictly Read-Only:** `xm-relay` never writes to agent databases/transcripts and never runs destructive Git operations (`reset`, `clean`, `checkout`, `stash`, `commit`).
- **Heuristic Secret Redaction:** The scanner in `xm-relay-security` is best-effort. **A clean scan is not proof that a package is free of secrets.** Always review `context.md` and `patches/working-tree.patch` before sharing a handoff package externally.
- **Untracked Files:** `working-tree.patch` contains tracked file diffs only. Untracked files are inventoried with size and SHA-256 in `repository-state.json` (and symlinks are never followed).
- **Antigravity CLI Transcripts:** Conversations without a JSONL transcript in `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/` (i.e. opaque SQLite blobs only) are skipped to uphold fail-safe parsing.

---

## 🧱 Workspace Crates

| Crate | Purpose |
|---|---|
| `crates/core` | Vendor-neutral domain types (`Session`, `Event`, `Evidence`, `TaskState`, `AgentAdapter`) |
| `crates/adapters/{opencode,antigravity,omp}` | Read-only session parsers + anonymized golden fixtures |
| `crates/repository` | Read-only Git inspection & project instruction discovery |
| `crates/security` | Heuristic secret detection and redaction |
| `crates/checkpoint` | Workspace checkpoint builder, checksum verifier, and drift comparator |
| `crates/handoff` | Evidence-based handoff engine, preflight validator, and prompt generator |
| `crates/tui` | Ratatui + Crossterm interactive terminal interface |
| `crates/cli` | `xm-relay` CLI binary |

---

## License

Released under the [MIT License](LICENSE).
