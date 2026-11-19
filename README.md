<div align="center">

# xmrelay

**Cross-agent session handoff & preflight verification for AI coding agents.**  
*Never lose work. Never silently lose context. Always verify before continuing.*

[![CI](https://github.com/iqbalmu/xmrelay/actions/workflows/ci.yml/badge.svg)](https://github.com/iqbalmu/xmrelay/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](rust-toolchain.toml)

[**Quick Install**](#-installation) • [**Quick Start**](#-quick-start) • [**CLI Reference**](#-cli-reference) • [**Workspace Crates**](#-workspace-crates)

</div>

---

## Why `xmrelay`?

When you hit a usage limit or switch models across AI coding agents (**OpenCode**, **Antigravity CLI**, **Oh My Pi**), copy-pasting the last few messages is unreliable. Agent summaries can hallucinate completed work, miss failed attempts, or contradict the actual state of your Git working tree.

`xmrelay` is a **local-first, strictly read-only** CLI tool that bridges coding agents safely:

1. **Discovers & Normalizes Sessions** across `opencode`, `antigravity-cli`, and `omp` without ever modifying source session databases or files.
2. **Inspects Live Git State** (branch, HEAD, staged/unstaged/untracked files, binary-safe diffs, and `AGENTS.md`/`CLAUDE.md` instructions) with `GIT_OPTIONAL_LOCKS=0` so `.git/index` is never mutated.
3. **Verifies Claims Against Disk:** Checks whether files reported as created/modified in a session actually exist in the workspace. Missing files or branch/HEAD drift trigger critical conflicts that **block continuation**.
4. **Redacts Secrets Automatically:** Scans transcripts, diffs, and project instructions for credentials (AWS, GitHub, Slack, Google, Stripe, `sk-` API keys, JWTs, private keys, URL credentials) and replaces them with `[REDACTED:<kind>]` before writing anything to disk.
5. **Enforces Target-Agent Preflight:** Generates a structured handoff package (`manifest.json`, `context.md`, `task-state.json`, `evidence.json`, `checksums.txt`) and a preflight prompt that requires the receiving agent to verify the workspace before writing code.

---

## 📦 Installation

### Option 1: One-Liner Script (Linux & macOS)
Downloads the latest prebuilt binary from GitHub Releases, verifies its SHA-256 checksum, and installs it to `~/.local/bin/xmrelay` (automatically falls back to `cargo install` if no prebuilt release is published yet):

```bash
curl -fsSL https://raw.githubusercontent.com/iqbalmu/xmrelay/main/install.sh | sh
```

<details>
<summary>Updating or customizing install directory</summary>

```bash
# Self-update to the latest release in-place:
xmrelay update

# Or customize the target directory during install:
curl -fsSL https://raw.githubusercontent.com/iqbalmu/xmrelay/main/install.sh | XMRELAY_INSTALL_DIR=/usr/local/bin sh
```
</details>

### Option 2: `cargo install` (From GitHub)
```bash
cargo install --git https://github.com/iqbalmu/xmrelay.git --locked xmrelay
```

### Option 3: `cargo-binstall`
```bash
cargo binstall --git https://github.com/iqbalmu/xmrelay xmrelay
```

### Option 4: Build from Source
```bash
git clone https://github.com/iqbalmu/xmrelay.git
cd xmrelay
cargo install --path crates/cli
```

---

## 🚀 Quick Start

### Interactive Arrow-Key Mode (Recommended)
Simply run `xmrelay` with no arguments inside your project directory:

```bash
xmrelay
```

Use **`↑`/`↓` (or `j`/`k`) + `Enter`** to navigate the streamlined interactive flow:
1. **Main Menu:** Choose **Hand Off a Session / Create Handoff** or **Previous Handoff History**.
2. **Pick Session:** Lists sessions in your active directory with human-friendly titles & relative timestamps. Use **`g`** to toggle Global scope, **`a`** to filter by agent, or **`/`** to search inline.
3. **Pick Target Agent:** Select `opencode`, `antigravity-cli` (`agy`), or `omp`.
4. **Immediate Action:** Copy the continuation prompt directly to your clipboard, print it to stdout, or launch the target agent CLI in the project directory with the prompt pre-loaded.

### Direct CLI Mode (For Scripting & Power Users)

```bash
# 1. Check detected agents and current repository status
xmrelay status

# 2. Find your latest session ID in the active directory (add -g for global)
xmrelay sessions list --agent opencode --limit 5

# 3. Build a verified, secret-redacted handoff package
xmrelay handoff create --from opencode --session <SESSION_ID> --to omp

# 4. Generate the preflight continuation prompt for the target agent
xmrelay continue <HANDOFF_ID>
```

> **Tip:** Pipe `xmrelay continue <HANDOFF_ID>` directly to your clipboard (`| wl-copy`, `| xclip -sel clip`, or `| pbcopy`) and paste it into your target coding agent. If a critical conflict is detected, `xmrelay continue` exits with code `2` and refuses to emit a prompt until the conflict is resolved.

---

## 🛠️ CLI Reference

Every subcommand supports `--help` and most provide `--json` for scripting.

| Command | Description |
|---|---|
| `xmrelay update` | Self-update in-place: downloads the release tarball, verifies its published SHA-256, swaps the binary atomically, keeps one `.xmrelay.bak` backup |
| `xmrelay agents` | Detect installed coding agents and their session data paths (`--json`) |
| `xmrelay status [path]` | Summary of detected agents, live Git state, checkpoints, and handoffs |
| `xmrelay sessions list` | List sessions for the active directory (`-g`/`--global` for all dirs, `--agent`, `--limit`, `--json`) |
| `xmrelay sessions search <query>` | Search sessions in the active directory (`-g`/`--global` for all dirs) |
| `xmrelay sessions inspect <id>` | Inspect normalized events and transparency `parse_report` |
| `xmrelay project inspect [path]` | Read-only Git status, diff stats, untracked inventory, and recent commits |
| `xmrelay project instructions [path]` | Discover `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `.cursorrules`, Copilot rules |
| `xmrelay project scan [path]` | Heuristic secret scan across diffs, untracked files, and instructions (exits `2` on hit) |
| `xmrelay checkpoint create [path]` | Snapshot Git status, instructions, and redacted patch with SHA-256 checksums |
| `xmrelay checkpoint list \| inspect \| diff` | List checkpoints, verify artifact checksums, or compare live workspace drift |
| `xmrelay handoff create` | Build a portable handoff package (`--from`, `--session`, `--to`, `--project`) |
| `xmrelay handoff list \| inspect \| validate` | View handoff history, inspect `context.md`, or re-validate against live disk |
| `xmrelay handoff export <id> <dest>` | Copy a checksum-verified handoff package to an external directory |
| `xmrelay history` | Shortcut to list recent handoff packages |
| `xmrelay continue <id>` | Re-validate and emit the target-agent preflight continuation prompt |

---

## 📂 Portable Handoff Package Layout

Packages are stored in `~/.local/share/xmrelay/handoffs/<handoff-id>/` (or `$XDG_DATA_HOME/xmrelay/handoffs/`) and have zero dependency on any internal database:

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

## Exit Codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Runtime error (bad path, unreadable data, download failure, ...) |
| `2` | Reserved for "needs attention": `project scan` found a potential secret; `handoff create`/`handoff validate`/`continue` hit a Blocked or Failed preflight; plus clap usage errors |

## 🔒 Security, Privacy & Known Limitations

- **Strictly Read-Only:** `xmrelay` never writes to agent databases/transcripts and never runs destructive Git operations (`reset`, `clean`, `checkout`, `stash`, `commit`).
- **Heuristic Secret Redaction:** The scanner in `xmrelay-security` is best-effort. **A clean scan is not proof that a package is free of secrets.** Always review `context.md` and `patches/working-tree.patch` before sharing a handoff package externally.
- **Continuation Prompt via argv:** Launching a target agent with a pre-loaded prompt passes it as a command-line argument, so it is visible system-wide via `ps`/`/proc/<pid>/cmdline` until the target process replaces it. This is a limitation of the target CLIs' flag-based prompt handoff; use "print to stdout" or "copy to clipboard" in shared environments.
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
| `crates/cli` | `xmrelay` CLI binary & interactive arrow-key wizard |

---

## License

Released under the [MIT License](LICENSE).
