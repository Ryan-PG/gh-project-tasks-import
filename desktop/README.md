# GitHub Importer — desktop app

A Tauri v2 desktop app that imports a `tasks.json` backlog into GitHub Issues
and a Projects V2 board.

It is a Rust port of [`import_tasks.py`](../import_tasks.py), not a wrapper
around it: **no Python and no `gh` CLI are needed to run it.** The Python CLI
stays in the repository as the reference implementation, and a parity oracle
diffs the two on every push.

> **Status:** the app is feature-complete and the port is verified against the
> CLI, but no signed release has been cut. See [Releasing](#releasing).

---

## Download and run

Each release carries installers plus a portable zip per platform. **Which one to
pick depends on the platform**, because "portable" means something different on
each:

| Platform | Best choice                      | Why                                                                                                                                        |
| -------- | -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| Windows  | `GitHub-Importer-portable-*.zip` | The zip is a bare `.exe` and Windows supplies the rest, so it genuinely needs nothing installed. Unzip anywhere writable and run it.       |
| macOS    | the `.dmg`                       | The zip holds a bare executable rather than an `.app`, which runs but has no proper bundle identity. The `.dmg` is the real portable form. |
| Linux    | the `.AppImage`                  | The zip holds a bare binary that still needs `webkit2gtk-4.1` from the system. The AppImage bundles it.                                    |

The portable zip is worth having on every platform because of what it contains
besides the executable: a `portable.txt` marker. With that present the app keeps
its settings and its import history **in the same folder**, so the folder can
move between machines or live on a USB stick with its state intact. Windows is
where that combination — no install, no runtime, settings included — pays off
most, so that is the recommended download there.

Installer builds store their settings in the per-user application config
directory instead.

### First-run prerequisites

| Platform   | What you may need                                                                                                                                                                                                                                                                                              |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Windows 10 | The [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/). Windows 11, and Windows 10 updated since 2022, already have it. The installer downloads the bootstrapper if it is missing; the **portable zip does not** — if the app exits immediately with no window, install the runtime. |
| macOS      | Nothing to install. The builds are **unsigned**, so Gatekeeper blocks the first launch: right-click the app → **Open** → **Open**. Double-clicking will not offer the option. If macOS still refuses, `xattr -dr com.apple.quarantine /path/to/GitHub\ Importer.app`.                                          |
| Linux      | `webkit2gtk-4.1` (the AppImage bundles what it can). On some NVIDIA and virtualised setups the window comes up blank — start it as `WEBKIT_DISABLE_DMABUF_RENDERER=1 ./github-importer`.                                                                                                                       |

---

## Signing in

The app never asks for your password, and it never stores one. There are three
ways in, in the order the sign-in screen offers them.

### 1. OAuth device flow (recommended)

Click **Sign in with GitHub**, and the app shows an eight-character code. Type
it at the URL shown. The app polls until you approve, up to a 15-minute window.

This needs an **OAuth App client id**. One is not currently bundled, so out of
the box the app asks you to paste one, or you can register your own — see
[Registering an OAuth App](#registering-an-oauth-app). Once a client id is
available it is remembered, and the sign-in screen skips straight to the code.

The device flow is capped by GitHub at **50 authorizations per hour across all
users of a given client id**. If you hit that, the app says so; wait, or use a
personal access token.

### 2. Personal access token

Paste a token instead. Either kind works:

- **Classic**, with the scopes `repo`, `project`, `read:org`. This is the safer
  choice for Projects V2.
- **Fine-grained**, granted _Issues: read and write_ on the target repository,
  plus the corresponding Projects permission. Be aware of two limits: GitHub's
  fine-grained support for user-owned Projects V2 boards is restricted, and
  fine-grained tokens **do not report scopes** — so the app cannot check them
  and will not warn about what is missing. A permission problem surfaces as an
  API error mid-run instead of as a warning up front.

### 3. Import from the `gh` CLI

If `gh` is installed and already authenticated, the app can adopt that session
so you do not have to sign in again. This is offered only when `gh` is found on
`PATH`; it is the migration path off the CLI, not a dependency of the app.

### Scopes

The app checks for `repo project read:org` and warns if any are missing.
`read:org` matters only when `project_owner` in `config.json` names an
organisation rather than your own account.

### "Remember me"

Ticked, the token is written to an encrypted `secrets.vault` file in the
workspace. Unticked, it is held in memory for the life of the process and
**nothing is written anywhere** — closing the app signs you out.

Set a vault passphrase and the vault additionally requires it, which is what
makes it resistant to another process running as you. Without a passphrase the
key is derived from a machine-bound identifier, which stops the vault file being
useful if it is copied to another machine or user account but not much else.

---

## Using the app

Four steps, left to right. Each unlocks the next.

1. **Sign in** — as above.
2. **Workspace** — the repository (`owner/name`), the project board name, and
   the project's owner. These are exactly the three keys of `config.json`.
3. **Backlog** — pick a `tasks.json`. The app validates it and shows a preview of
   every issue the run would create. Validation mirrors the CLI line for line:
   a backlog the CLI accepts is accepted here, and one it rejects is rejected
   with the same messages in the same order.
4. **Import** — choose dry-run, skip-relationships, or skip-project, then run.
   Progress streams live. **Compare with the CLI** re-runs the reference
   implementation's preview and shows both outputs side by side.

The import is **idempotent and resumable**. Re-running it will not duplicate
anything: existing issues are recognised by the frozen title pattern
(`[ABC-123] …`) and a body marker, including issues created by the Python CLI.
If a run dies partway, start it again — it picks up from where it stopped. The
`.import-state.json` file is only a cache; correctness comes from re-reading
GitHub, so deleting it costs time and nothing else.

---

## Portable mode

Portable mode is switched on by a marker file **next to the executable**:

- `portable.txt`, or
- `.portable`

With either present, `config.json`, `tasks.json`, `.env`, `.import-state.json`
and `secrets.vault` all live in that directory. Without one, they live in the
per-user application config directory:

| Platform | Default location                                         |
| -------- | -------------------------------------------------------- |
| Windows  | `%APPDATA%\com.github.task-importer`                     |
| macOS    | `~/Library/Application Support/com.github.task-importer` |
| Linux    | `~/.config/com.github.task-importer`                     |

The **Workspace** step displays the resolved paths, so you can always see which
mode is active and where the app is reading from.

The shipped portable zip includes the marker. Delete it to go back to per-user
storage.

---

## Configuration

`config.json` is shared byte-for-byte with the CLI:

```json
{
  "repo": "OWNER/API",
  "project": "API",
  "project_owner": "OWNER"
}
```

Two environment switches from the CLI are honoured, read from the process
environment and then from a `.env` file beside the executable:

| Variable              | Default  | Effect                                                                                |
| --------------------- | -------- | ------------------------------------------------------------------------------------- |
| `RELATIONSHIP_ERRORS` | `strict` | `ignore` downgrades parent/sub-issue and dependency failures from errors to warnings. |
| `SKIP_RELATIONSHIPS`  | `false`  | `true` skips the relationships phase entirely.                                        |

A value that is neither of the two accepted spellings is reported in the
**Backlog** step, and on stderr by the CLI — the setting keeps its default.

---

## Building and running from source

Prerequisites: Rust stable (MSVC toolchain on Windows), **Node 20.19+ or
22.12+** — what Vite 7 supports — and the
[Tauri v2 system dependencies](https://tauri.app/start/prerequisites/) for your
platform.

```bash
git clone https://github.com/<owner>/gh-project-tasks-import.git
cd gh-project-tasks-import/desktop
npm install
npm run tauri:dev      # run with hot reload
npm run tauri:build    # produce installers and bundles
```

The first `tauri:dev` compiles the Rust side and takes a few minutes; after that
it is incremental and the frontend hot-reloads.

### Where a dev build reads its files

`config.json` and `tasks.json` are **not in git** — they are your workspace. A
fresh clone therefore has neither, and the app starts with an empty workspace
aimed at the per-user config directory.

A debug build resolves its workspace by **walking up from where the app starts,
looking for a `config.json` or `tasks.json`**. Create one at the repo root and
the dev app adopts the repo root, sharing `config.json`, `tasks.json` and
`.import-state.json` with the Python CLI:

```bash
cd ..                                  # the repo root, not desktop/
cp config.example.json config.json
```

Resolution happens once at startup, so create the file before launching — or
restart afterwards. Skip it and the app still runs; the **Workspace** step takes
the three values by hand and saves them to the per-user config directory. Either
way that step shows the resolved paths, which is the quickest way to confirm
which directory it landed on. To exercise portable mode in a dev build instead,
drop a `portable.txt` beside the compiled binary in `src-tauri/target/debug/`.

Bake in a default client id for the device flow by setting it at build time:

```bash
GITHUB_CLIENT_ID=Iv1.0123456789abcdef npm run tauri:build
```

Precedence is: this build-time value, then a client id saved in the app's
settings, then one typed into the sign-in screen.

### Checks

```bash
npm run typecheck      # tsc --noEmit
npm run test:rust      # cargo test --all-targets
npm run parity         # diff the port against import_tasks.py
npm run check          # all three, with the parity binary rebuilt first
```

`npm run parity` needs the `parity` binary to exist; `npm run parity:build`
builds it first. These npm scripts call `python`, which is the right name for
local Windows development. On Linux and macOS, where only `python3` exists, run
the scripts directly — `python3 scripts/parity_oracle.py` — or substitute the
interpreter yourself. CI uses `python3` throughout.

### The parity oracle

`scripts/parity_oracle.py` is the reason to believe this port. It runs
`import_tasks.py` and the Rust `parity` binary over the fixture corpus in
`scripts/fixtures/`, across a matrix of settings environments, and requires
byte-identical stdout, byte-identical stderr, and matching exit status.

```
python scripts/parity_oracle.py --build      # rebuild the binary first
python scripts/parity_oracle.py --only valid-relationships
```

The corpus is generated by `scripts/make_fixtures.py`, which also copies the
repository's real `tasks.json` in as a fixture. CI regenerates the corpus and
fails if the committed copy differs.

Three fixtures are deliberately **divergent** — `divergent-missing-id`,
`divergent-missing-parent`, `divergent-missing-depends-on`. In each, the Python
CLI crashes with a `KeyError` traceback while the port reports a clean
validation error. That is a deliberate improvement, and the oracle asserts that
the divergence stays exactly this shape, so the list cannot silently go stale.

---

## Registering an OAuth App

The device flow needs an OAuth App. This is a **one-time, manual step and a hard
prerequisite** — GitHub has no API to register one.

1. Go to **Settings → Developer settings → OAuth Apps → New OAuth App**.
   For an organisation-owned app, use the organisation's developer settings
   instead.
2. Fill in the form. Only the name is really used:
   - **Application name:** anything, e.g. `GitHub Importer`
   - **Homepage URL:** your repository URL
   - **Authorization callback URL:** `http://localhost` — the device flow does
     not use it, but the field is required.
3. On the app's page, click **Enable device flow**. This is the step people
   miss; without it GitHub rejects every device-flow request.
4. Copy the **Client ID**. There is no client secret to copy — the device flow
   does not use one, which is what makes it safe to ship a client id in a
   desktop binary.

Then either paste the client id into the sign-in screen, or build with it baked
in (see above). For CI, set it as the `GITHUB_CLIENT_ID` repository secret.

---

## Releasing

`.github/workflows/desktop-release.yml` builds a matrix of `windows-latest`,
`macos-latest` (universal binary) and `ubuntu-22.04`, and a portable zip per
platform. Ubuntu 22.04 is deliberate: Tauri v2 links `webkit2gtk-4.1`, and a
binary built on 24.04 will not start on 22.04.

Tag to release:

```bash
git tag desktop-v0.1.0
git push origin desktop-v0.1.0
```

That creates a **draft** release — review the assets and publish it by hand.
A manual `workflow_dispatch` run builds and uploads artifacts without touching
releases.

Set the `GITHUB_CLIENT_ID` repository secret before tagging, or the shipped
binaries will ask every user for a client id.

### Known gaps

- **The macOS builds are unsigned and unnotarised.** This is a deliberate
  decision to avoid a paid Apple Developer account; the consequence is the
  Gatekeeper dance described above. If that becomes unacceptable, the fix is an
  Apple Developer ID plus notarisation in the workflow.
- **No auto-updater.** `createUpdaterArtifacts` is off in `tauri.conf.json`.
  Updates are a manual download.
- **No code signing on Windows either**, so SmartScreen will warn on first run
  of the installer.

---

## Implementation notes

### Why not Stronghold

The plan specified `tauri-plugin-stronghold` for secret storage. It is not used.
Stronghold depends on `libsodium-sys`, whose build script downloads a prebuilt C
library over plain HTTP **at compile time**. That failed here with a network
timeout, and it is a problem independent of this machine: it breaks offline
builds and makes the CI matrix non-reproducible.

Instead, `src/secrets.rs` implements a small vault directly — Argon2id for key
derivation, ChaCha20-Poly1305 for the payload, both pure Rust — behind a
`SecretStore` trait with `MemoryStore` and `EncryptedFileVault` implementations.
Dropping Stronghold back in means adding a third implementation of that trait;
no caller changes.

The trade is that this vault is **not** independently audited, unlike
libsodium. Its guarantees, stated plainly, are in the module doc comment, and
they are narrower than Stronghold's: without a user passphrase it protects
against a copied vault file, not against malware running as the same user.

### Two binaries in one crate

`src/bin/parity.rs` builds a second executable from the same crate, and Tauri
has to decide which one is the app. With two candidates and nothing marked
main it fell through to the first entry it had collected — `parity` — so the
installer was assembled around a 227 KB command-line tool. It built cleanly,
uploaded cleanly, and installed an app that never opened a window.

Three settings keep it correct. The first two are load-bearing:

- **`default-run = "github-importer"`** in `Cargo.toml`. Tauri marks the app
  binary by matching a target against the package name _or_ `default-run`, and
  only the latter compares against the target name — the package-name check
  compares the source file stem (`main`), which never matches.
- **`required-features = ["oracle"]`** on the `parity` bin. Without it the
  oracle counts as a second bundle binary and is installed next to the app,
  even once the app itself is selected correctly.
- `"mainBinaryName": "github-importer"` in `tauri.conf.json` keeps the output
  name stable and free of the space in `productName`. It does **not** influence
  which binary is picked — setting it alone was not enough, and while it was
  the only fix the bundle still contained the oracle.

Build the oracle with `cargo build --bin parity --features oracle`
(`npm run parity:build` does this for you).

### Layout

```
desktop/
  src/                     React + TypeScript frontend
    components/            one panel per step, plus the shared ui primitives
    lib/api.ts             typed wrappers over the IPC commands
    lib/types.ts           mirrors of the Rust DTOs
  src-tauri/src/
    tasks.rs               model, validation, preview — the port of the CLI
    github.rs              REST client: pagination, retry, labels, issues
    graphql.rs             Projects V2
    auth.rs                device flow, PAT, gh import
    secrets.rs             the vault
    importer.rs            the two-pass import engine
    commands.rs            the IPC surface
    state.rs               portable-aware paths, resumable state
    bin/parity.rs          the oracle's Rust side
  scripts/                 fixture generation and the parity oracle
```

Secrets live **only in the Rust process**. The webview never sees a token. Every
GitHub call is made in Rust and exposed to the frontend as an IPC command, and
the CSP in `tauri.conf.json` restricts the webview to `'self'` plus the IPC
channel.
