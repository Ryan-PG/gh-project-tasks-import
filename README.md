<p align="center">
  <img
    src="https://ishan-rest.vercel.app/svg/banner/dev/GitHub_Project_Tasks_Importer"
    alt="GitHub_Project_Tasks_Importer"
    width="100%"
  />
</p>

<h1 align="center">GitHub Project Tasks Importer</h1>

<p align="center">
  Import structured task backlogs into GitHub Issues and Projects.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Python-3.10%2B-3776AB?logo=python&logoColor=white">
  <img src="https://img.shields.io/badge/GitHub_CLI-required-181717?logo=github&logoColor=white">
  <img src="https://img.shields.io/badge/GitHub_Projects-supported-181717?logo=github&logoColor=white">
  <img src="https://img.shields.io/badge/License-MIT-green">
</p>

## Two ways to run it

**The Python CLI** — [`import_tasks.py`](import_tasks.py), documented in this
file. It is the reference implementation, and it needs Python 3.10+ and the
`gh` CLI.

**The desktop app** — [`desktop/`](desktop/), documented in
[`desktop/README.md`](desktop/README.md). A Rust port of the same logic — same
validation, same idempotency, same `config.json` and `.import-state.json` —
packaged as a downloadable app that needs neither Python nor `gh`. It can adopt
an existing `gh` session, so migrating is a single click, and it ships as a
portable zip that runs from a USB stick.

Both are supported. A parity oracle diffs the two on every push, so a backlog
accepted by one is accepted by the other.

### Running the desktop app from a clone

**Prerequisites.** Rust stable via [rustup](https://rustup.rs/) — on Windows
choose the MSVC toolchain and add the "Desktop development with C++" workload
from the Visual Studio Build Tools. Node.js **20.19+ or 22.12+**, which is what
Vite 7 supports. Plus the system libraries Tauri needs:

- **Windows** — nothing beyond the above. WebView2 ships with Windows 11 and
  with Windows 10 updated since 2022.
- **macOS** — `xcode-select --install`
- **Linux** — `libwebkit2gtk-4.1-dev`, `build-essential`, `curl`, `wget`,
  `file`, `libxdo-dev`, `libssl-dev`, `libayatana-appindicator3-dev`,
  `librsvg2-dev`

**Clone and run:**

```bash
git clone https://github.com/<owner>/gh-project-tasks-import.git
cd gh-project-tasks-import/desktop
npm install
npm run tauri:dev
```

The first run compiles the Rust side and takes a few minutes. After that it is
incremental, and the frontend hot-reloads on save.

**Point it at a workspace before you start.** `config.json` and `tasks.json`
are deliberately **not in git** — they are your workspace, not the project's —
so a fresh clone has neither, and the app comes up with an empty workspace
aimed at your per-user config directory. A debug build resolves its workspace
by **walking up from where the app starts, looking for a `config.json` or
`tasks.json`**. So create one at the repo root and the dev app will use the
repo root as its workspace — the same `config.json`, `tasks.json` and
`.import-state.json` the Python CLI uses:

```bash
cd ..                                   # back to the repo root
cp config.example.json config.json
$EDITOR config.json                     # repo, project, project_owner
```

That resolution happens once, at startup, so create the file _before_ launching
— or restart after creating it. The **Workspace** step always displays the
resolved paths, so you can confirm which one it found.

A release build, or a debug build with no such file anywhere up the tree, uses
the per-user application config directory instead. To exercise portable mode in
a dev build, drop a `portable.txt` beside the compiled binary in
`desktop/src-tauri/target/debug/`; see
[Portable mode](desktop/README.md#portable-mode) for what that changes.

**Signing in** works exactly as it does in a release build — device flow, a
personal access token, or **Import from `gh`** if you are already authenticated
with the CLI, which is the quickest route on a dev machine. If you use the
device flow you will need a client id; paste one into the sign-in screen or see
[Registering an OAuth App](desktop/README.md#registering-an-oauth-app).

Other things you can run from `desktop/`:

```bash
npm run check          # typecheck + Rust tests + the parity oracle
npm run tauri:build    # produce installers and bundles
npm run typecheck      # tsc --noEmit
npm run test:rust      # cargo test --all-targets
npm run parity         # diff the port against import_tasks.py
```

`desktop/README.md` has the rest: the [parity
oracle](desktop/README.md#the-parity-oracle), [OAuth App
registration](desktop/README.md#registering-an-oauth-app), and
[releasing](desktop/README.md#releasing).

## Requirements

For the Python CLI:

- Python 3.10+
- GitHub CLI (`gh`)
- Permission to create issues in the target repository
- An existing GitHub Project

## Install GitHub CLI

### Windows

Using `winget`:

```powershell
winget install --id GitHub.cli
```

Or install manually from:

[https://cli.github.com/](https://cli.github.com/)

### macOS

Using Homebrew:

```bash
brew install gh
```

### Linux

Debian/Ubuntu:

```bash
sudo apt update
sudo apt install gh
```

Other distributions:

[https://github.com/cli/cli#installation](https://github.com/cli/cli#installation)

Verify:

```bash
gh --version
```

## Authenticate

```bash
gh auth login
```

For GitHub Project operations, also enable the `project` scope:

```bash
gh auth refresh -s project
```

Verify:

```bash
gh auth status
```

## Find Your Projects

List projects you can access:

```bash
gh project list --owner YOUR_USERNAME
```

For an organization:

```bash
gh project list --owner YOUR_ORG
```

Make sure the Project name in `config.json` exactly matches an existing Project.

Example:

```json
{
  "repo": "PersianRepo/API",
  "project": "API",
  "project_owner": "PersianRepo"
}
```

> `project_owner` is the GitHub user or organization that owns the Project, not necessarily the repository owner.

## Import Tasks

1. Edit `config.json`.
2. Validate the task file:

```bash
python import_tasks.py validate
```

3. Preview what will be created:

```bash
python import_tasks.py preview
```

4. Create/import everything:

```bash
python import_tasks.py create
```

## Rerun Safely

The importer is idempotent.

It identifies managed issues using task IDs such as:

```text
[AUTH-001]
```

Rerunning the importer does not create duplicate issues.

It will:

- Create missing labels
- Create missing issues
- Add issues to the configured Project
- Apply parent/dependency relationships
- Track imported issues in `.import-state.json`

## Configuration (.env)

Optional settings are read from a `.env` file in the repo root, next to `config.json`. None are required — the defaults reproduce the original behaviour and the file does not have to exist.

```bash
cp .env.example .env
```

Shell environment variables take precedence over `.env`, so CI can override without editing files.

| Key                   | Values               | Default  | Effect                                                                                                                                  |
| --------------------- | -------------------- | -------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `RELATIONSHIP_ERRORS` | `strict` \| `ignore` | `strict` | `strict` aborts on the first parent/blocked-by failure. `ignore` tolerates links that are already set and continues through every task. |
| `SKIP_RELATIONSHIPS`  | `true` \| `false`    | `false`  | `true` skips the parent/blocked-by pass entirely. Issues are still created; no links are applied.                                       |

The effective settings and where each value came from (`default`, `.env`, or `environment`) are printed on startup, so you can confirm what was picked up.

### Rerunning after a partial failure

The relationship pass re-applies every parent and blocked-by link on every run. On a rerun, GitHub rejects links that already exist:

```text
GraphQL: An error occurred while adding the blocking issue to the issue.
Validation failed: Target issue has already been taken (addBlockedBy)
```

In `strict` mode that aborts the pass, stranding every remaining task even though the desired state already exists on GitHub. Set `ignore` to finish the pass:

```text
RELATIONSHIP_ERRORS=ignore
```

The run then reaches every task and reports what happened:

```text
Already set (5): AUTH-FE-003 (parent), USER-FE-001 (blocked-by), ...
Relationships: 84 applied, 5 already set, 0 failed.
```

> **`ignore` mode can mask genuine failures.** Permission errors, deleted issues, and rate limits are printed as warnings but no longer stop the pass. Check the `Failed (n)` line before assuming a run was clean. Switch back to `strict` once the backlog is in a known-good state.

## Troubleshooting

### Project not found

Check the Project owner and name:

```bash
gh project list --owner YOUR_USERNAME
```

Then make sure `config.json` uses the exact Project name.

### Permission errors

Re-authenticate with the Project scope:

```bash
gh auth refresh -s project
gh auth status
```

You also need permission to create issues in the target repository.

### Python command not found

On Linux and macOS the interpreter is usually `python3`, not `python`:

```bash
python3 import_tasks.py validate
```

On Windows, `python` is the right name. If neither resolves, install Python
3.9+ and reopen your terminal so `PATH` picks it up.

### `cargo metadata` — "program not found"

Running `npm run tauri:dev` fails with:

```
failed to run 'cargo metadata' command to get workspace directory:
failed to run command cargo metadata --no-deps --format-version 1: program not found
```

This means Tauri could not find `cargo`, not that Rust is broken. It is almost
always a **stale `PATH`**: rustup appends `%USERPROFILE%\.cargo\bin` to your
user environment, but Windows gives every process a *copy* of `PATH` when it
starts, so terminals that were already open never see the new entry.

Check whether the toolchain is actually installed:

```powershell
& "$env:USERPROFILE\.cargo\bin\cargo.exe" --version
```

If that prints a version, **open a new terminal** and run `npm run tauri:dev`
again. To fix the current session instead of restarting it:

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
```

If the command above reports that the file does not exist, Rust is genuinely
missing — install it from [rustup.rs](https://rustup.rs/) and reopen your
terminal.

The desktop app needs Node **20.19+ or 22.12+**, which is what Vite 7 supports;
`node --version` will tell you which you have.
