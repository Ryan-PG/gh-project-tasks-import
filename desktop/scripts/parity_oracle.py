#!/usr/bin/env python3
"""Parity oracle: prove the Rust port is indistinguishable from the CLI.

`import_tasks.py` is the reference implementation and stays the source of
truth until the CLI is retired. This script runs both it and the Rust `parity`
binary over the fixture corpus, in a matrix of settings environments, and
requires byte-identical stdout, byte-identical stderr, and the same exit
status.

The Python reference locates `tasks.json` and `.env` relative to its own file,
so each case runs inside a throwaway directory holding a copy of the script
plus that case's inputs. Nothing in the working tree is touched.

    python scripts/parity_oracle.py            # build is assumed to exist
    python scripts/parity_oracle.py --build    # run cargo build --bin parity --features oracle first

Exit status is 0 when every case matches, 1 otherwise.
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# Fixture names and diffs contain non-ASCII, and the default console encoding on
# Windows is cp1252 — which cannot encode the very mismatches this script exists
# to print.
for stream in (sys.stdout, sys.stderr):
    if hasattr(stream, "reconfigure"):
        stream.reconfigure(encoding="utf-8", errors="replace")

HERE = Path(__file__).resolve().parent
DESKTOP = HERE.parent
REPO = DESKTOP.parent

CLI = REPO / "import_tasks.py"
CONFIG = REPO / "config.json"
FIXTURES = HERE / "fixtures"
PARITY_BIN = DESKTOP / "src-tauri" / "target" / "debug" / (
    "parity.exe" if os.name == "nt" else "parity"
)

# The CLI's `main()` loads `config.json` before it looks at any command, so the
# reference needs one present even for `validate`. Its contents do not affect
# validate or preview, which is why the oracle can run without a real workspace.
FALLBACK_CONFIG = {
    "repo": "example/repo",
    "project": "Example",
    "project_owner": "example",
}

COMMANDS = ("validate", "preview")

# Fixtures whose Python run is expected to raise rather than report. See the
# bottom of make_fixtures.py.
DIVERGENT_PREFIX = "divergent-"

# Settings environments, applied to both implementations identically. `env` is
# overlaid on the ambient environment; `dotenv` is written as `.env`.
SCENARIOS: list[dict] = [
    {"name": "default", "env": {}, "dotenv": None},
    {"name": "ignore", "env": {"RELATIONSHIP_ERRORS": "ignore"}, "dotenv": None},
    {"name": "skip-rel", "env": {"SKIP_RELATIONSHIPS": "true"}, "dotenv": None},
    {
        "name": "both-from-env",
        "env": {"RELATIONSHIP_ERRORS": "ignore", "SKIP_RELATIONSHIPS": "true"},
        "dotenv": None,
    },
    {
        "name": "from-dotenv",
        "env": {},
        "dotenv": "RELATIONSHIP_ERRORS=ignore\nSKIP_RELATIONSHIPS=true\n",
    },
    {
        "name": "env-beats-dotenv",
        "env": {"RELATIONSHIP_ERRORS": "strict"},
        "dotenv": "RELATIONSHIP_ERRORS=ignore\nSKIP_RELATIONSHIPS=TRUE\n",
    },
    # Invalid values: the CLI warns on stderr and falls back to the default,
    # annotating the source. The warning text is the fiddly part.
    {
        "name": "invalid-value",
        "env": {"RELATIONSHIP_ERRORS": "sometimes"},
        "dotenv": None,
    },
    {
        "name": "invalid-both",
        "env": {"RELATIONSHIP_ERRORS": "nope", "SKIP_RELATIONSHIPS": "yes"},
        "dotenv": None,
    },
    # Case and surrounding whitespace are normalised by `.strip().lower()`.
    {
        "name": "case-and-space",
        "env": {"RELATIONSHIP_ERRORS": "  IGNORE  ", "SKIP_RELATIONSHIPS": " True "},
        "dotenv": None,
    },
    # A quoted value in .env, which `load_dotenv` strips.
    {
        "name": "dotenv-quoted",
        "env": {},
        "dotenv": 'RELATIONSHIP_ERRORS="ignore"\nSKIP_RELATIONSHIPS=\'true\'\n',
    },
    # Comments and blank lines must be ignored the same way.
    {
        "name": "dotenv-comments",
        "env": {},
        "dotenv": "# a comment\n\nRELATIONSHIP_ERRORS=ignore\n   \nNOT_A_SETTING=1\n",
    },
]


def run(cmd: list[str], cwd: Path, env: dict[str, str]) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        text=True,
        encoding="utf-8",
        capture_output=True,
    )


def diff(label: str, expected: str, actual: str) -> list[str]:
    return list(
        difflib.unified_diff(
            expected.splitlines(keepends=True),
            actual.splitlines(keepends=True),
            fromfile=f"python/{label}",
            tofile=f"rust/{label}",
        )
    )


def as_divergent_regression(name: str, py, rs) -> list[str]:
    """Check a known-divergence case still diverges in the expected way."""
    problems = []
    if py.returncode == 0:
        problems.append(
            f"{name}: Python now succeeds, so the recorded divergence is stale — "
            f"drop the {DIVERGENT_PREFIX} prefix and let it be compared normally."
        )
    if "Traceback" not in py.stderr:
        problems.append(
            f"{name}: Python failed without a traceback, so it is no longer a "
            f"crash case. stderr was:\n{py.stderr}"
        )
    # The Rust side must fail cleanly, with the structural error and no panic.
    if rs.returncode == 0:
        problems.append(f"{name}: the Rust port succeeded where Python crashed.")
    if "panicked" in rs.stderr:
        problems.append(f"{name}: the Rust port panicked:\n{rs.stderr}")
    if "Validation failed:" not in rs.stdout:
        problems.append(
            f"{name}: the Rust port did not report a validation failure. stdout:\n{rs.stdout}"
        )
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--build",
        action="store_true",
        help="run `cargo build --bin parity` before comparing",
    )
    ap.add_argument(
        "--only",
        action="append",
        default=[],
        help="restrict to fixtures whose name contains this substring (repeatable)",
    )
    args = ap.parse_args()

    if not CLI.is_file():
        print(f"reference implementation not found: {CLI}", file=sys.stderr)
        return 2

    if args.build:
        cargo = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo")
        print(f"building {PARITY_BIN.name}…")
        # The oracle bin is behind the `oracle` feature so Tauri does not ship it
        # inside the app's installers.
        built = subprocess.run(
            [cargo, "build", "--bin", "parity", "--features", "oracle"],
            cwd=DESKTOP / "src-tauri",
        )
        if built.returncode != 0:
            return built.returncode

    if not PARITY_BIN.is_file():
        print(
            f"parity binary not found: {PARITY_BIN}\n"
            f"build it first:  cargo build --bin parity --features oracle"
            f"   (in {DESKTOP / 'src-tauri'})",
            file=sys.stderr,
        )
        return 2

    fixtures = sorted(FIXTURES.glob("*.json"))
    if args.only:
        fixtures = [f for f in fixtures if any(s in f.stem for s in args.only)]
    if not fixtures:
        print("no fixtures found; run scripts/make_fixtures.py", file=sys.stderr)
        return 2

    passed = 0
    failed: list[tuple[str, str, list[str]]] = []

    for fixture in fixtures:
        divergent = fixture.stem.startswith(DIVERGENT_PREFIX)
        for command in COMMANDS:
            for scenario in SCENARIOS:
                label = f"{fixture.stem} / {command} / {scenario['name']}"

                with tempfile.TemporaryDirectory(prefix="parity-") as tmp:
                    work = Path(tmp)
                    shutil.copy2(CLI, work / "import_tasks.py")
                    shutil.copy2(fixture, work / "tasks.json")
                    if CONFIG.is_file():
                        shutil.copy2(CONFIG, work / "config.json")
                    else:
                        (work / "config.json").write_text(
                            json.dumps(FALLBACK_CONFIG, indent=2) + "\n", encoding="utf-8"
                        )
                    if scenario["dotenv"] is not None:
                        (work / ".env").write_text(scenario["dotenv"], encoding="utf-8")

                    env = dict(os.environ)
                    # Ambient values would otherwise leak in and change the
                    # precedence each case is meant to exercise.
                    env.pop("RELATIONSHIP_ERRORS", None)
                    env.pop("SKIP_RELATIONSHIPS", None)
                    env.update(scenario["env"])
                    env["PYTHONIOENCODING"] = "utf-8"

                    py = run([sys.executable, str(work / "import_tasks.py"), command], work, env)

                    rust_cmd = [str(PARITY_BIN), command, str(work / "tasks.json")]
                    if scenario["dotenv"] is not None:
                        rust_cmd += ["--dotenv", str(work / ".env")]
                    rs = run(rust_cmd, work, env)

                if divergent:
                    problems = as_divergent_regression(label, py, rs)
                else:
                    problems = []
                    if py.returncode != rs.returncode:
                        problems.append(
                            f"exit status differs: python={py.returncode} rust={rs.returncode}"
                        )
                    if py.stdout != rs.stdout:
                        problems.append("stdout differs:\n" + "".join(
                            diff("stdout", py.stdout, rs.stdout)
                        ))
                    if py.stderr != rs.stderr:
                        problems.append("stderr differs:\n" + "".join(
                            diff("stderr", py.stderr, rs.stderr)
                        ))

                if problems:
                    failed.append((label, fixture.name, problems))
                    print(f"FAIL  {label}")
                else:
                    passed += 1
                    print(f"ok    {label}")

    total = passed + len(failed)
    print()
    if failed:
        print(f"{len(failed)} of {total} cases failed.\n")
        for label, _, problems in failed:
            print("=" * 72)
            print(label)
            print("=" * 72)
            for p in problems:
                print(p)
            print()
        return 1

    print(f"All {total} cases match: stdout, stderr, and exit status.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
