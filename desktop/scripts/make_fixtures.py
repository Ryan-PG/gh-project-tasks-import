#!/usr/bin/env python3
"""Generate the parity fixture corpus in `scripts/fixtures/`.

The corpus exists so the parity oracle can prove the Rust port and
`import_tasks.py` agree. Each fixture is a `tasks.json` in the sense the CLI
understands: a JSON array of task objects.

Fixtures are checked in, so this script is only re-run when the corpus itself
changes. It is kept next to the oracle so the intent of each case stays
readable.

    python scripts/make_fixtures.py
"""

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "fixtures"


def task(tid, **over):
    """A complete, valid task, with fields overridable or removable."""
    base = {
        "id": tid,
        "module": "backend",
        "title": f"Task {tid}",
        "priority": "P1",
        "labels": [],
        "depends_on": [],
        "parent": None,
        "body": f"Body for {tid}.",
    }
    for key, value in over.items():
        if value is _DELETE:
            base.pop(key, None)
        else:
            base[key] = value
    return base


class _Delete:
    def __repr__(self):
        return "DELETE"


_DELETE = _Delete()


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    fixtures = {}

    # --- Accepted inputs ----------------------------------------------------
    fixtures["valid-minimal"] = [task("DOC-FE-001")]

    fixtures["valid-empty-list"] = []

    fixtures["valid-relationships"] = [
        task("DOC-FE-001"),
        task("DOC-FE-002", parent="DOC-FE-001", depends_on=["DOC-FE-001"]),
        task("DOC-FE-003", parent="DOC-FE-002", depends_on=["DOC-FE-001", "DOC-FE-002"]),
    ]

    # The preview row pads id to 15 and module to 18; these sit exactly on and
    # just past both boundaries, which is where an off-by-one would show.
    fixtures["valid-padding-boundaries"] = [
        task("ABCDEFGHIJKLMNO"),                      # id: exactly 15
        task("ABCDEFGHIJKLMNOP"),                     # id: 16
        task("AB-1", module="A" * 18),                # module: exactly 18
        task("AB-2", module="B" * 19),                # module: 19
        task("AB-3", module=""),                      # module: empty
        task("AB-4", title=""),                       # title: empty
    ]

    fixtures["valid-unicode"] = [
        task("DOC-FE-001", title="Fix the — em dash and “smart quotes”", module="фронтенд"),
        task("DOC-FE-002", title="絵文字 🎉 and CJK 日本語", labels=["バグ", "🎯"]),
        task("DOC-FE-003", title="combining é vs é", body="ünïcödé"),
    ]

    fixtures["valid-extra-fields"] = [
        {**task("DOC-FE-001"), "assignee": "octocat", "estimate": 3, "nested": {"a": [1, 2]}},
    ]

    fixtures["valid-falsy-parent"] = [
        # Python's `if t["parent"] and ...` treats every falsy value as "no
        # parent", so none of these may be reported as unknown.
        task("DOC-FE-001", parent=0),
        task("DOC-FE-002", parent=False),
        task("DOC-FE-003", parent=""),
        task("DOC-FE-004", parent=[]),
        task("DOC-FE-005", parent={}),
        task("DOC-FE-006", parent=None),
    ]

    fixtures["valid-p0-through-p3"] = [
        task("DOC-FE-001", priority="P0"),
        task("DOC-FE-002", priority="P1"),
        task("DOC-FE-003", priority="P2"),
        task("DOC-FE-004", priority="P3"),
    ]

    # --- Rejected inputs ----------------------------------------------------
    fixtures["invalid-duplicate-id"] = [
        task("DOC-FE-001"),
        task("DOC-FE-001", title="A second one with the same id"),
        task("DOC-FE-001", title="And a third"),
    ]

    fixtures["invalid-priority"] = [
        task("DOC-FE-001", priority="P4"),
        task("DOC-FE-002", priority="p0"),
        task("DOC-FE-003", priority=""),
        task("DOC-FE-004", priority=1),
        task("DOC-FE-005", priority=None),
    ]

    # Only fields that pass 2 never touches, so the Python reference reaches its
    # own error report instead of raising KeyError.
    fixtures["invalid-missing-fields"] = [
        task("DOC-FE-001", module=_DELETE, title=_DELETE, priority=_DELETE,
             labels=_DELETE, body=_DELETE),
    ]

    fixtures["invalid-unknown-dependency"] = [
        task("DOC-FE-001", depends_on=["DOC-FE-999"]),
        task("DOC-FE-002", depends_on=["DOC-FE-001", "NOPE-1", "NOPE-2"]),
    ]

    fixtures["invalid-self-dependency"] = [
        task("DOC-FE-001", depends_on=["DOC-FE-001"]),
        # Known id, so only the self-dependency error applies — not also
        # "unknown dependency".
        task("DOC-FE-002", depends_on=["DOC-FE-002", "DOC-FE-001"]),
    ]

    fixtures["invalid-unknown-parent"] = [
        task("DOC-FE-001", parent="DOC-FE-999"),
        task("DOC-FE-002"),
    ]

    # Pass 1 must report every task's structural errors before pass 2 reports
    # any relationship error. Interleaving them here would expose a port that
    # checked relationships inline.
    fixtures["invalid-pass1-before-pass2"] = [
        task("DOC-FE-001", priority="P9", depends_on=["MISSING-1"]),
        task("DOC-FE-002", priority="P8", parent="MISSING-2"),
        task("DOC-FE-003", depends_on=["MISSING-3"]),
    ]

    fixtures["invalid-string-depends-on"] = [
        # A plausible typo. Python iterates the string, one error per character.
        task("DOC-FE-001", depends_on="DOC-FE-002"),
        task("DOC-FE-002"),
    ]

    for name, payload in fixtures.items():
        path = OUT / f"{name}.json"
        path.write_text(
            json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        print(f"wrote {path.relative_to(HERE.parent)}")

    # The real backlog, verbatim. It is the single most valuable case: a large,
    # hand-maintained file with every shape the schema allows, which is exactly
    # what the port has to keep working.
    real = HERE.parent.parent / "tasks.json"
    if real.is_file():
        target = OUT / "valid-real-backlog.json"
        target.write_bytes(real.read_bytes())
        print(f"wrote {target.relative_to(HERE.parent)}  (copied from {real.name})")
    else:
        print(f"! {real} not found; skipping the real-backlog fixture")

    # --- Known divergences --------------------------------------------------
    # These crash the Python reference with a KeyError, so there is no output to
    # match. The Rust port reports the structural error and exits 1 instead.
    # `parity_oracle.py` asserts Python still crashes, so this list cannot go
    # stale silently.
    divergences = {
        "divergent-missing-id": [task("DOC-FE-001"), task("DOC-FE-002", id=_DELETE)],
        "divergent-missing-depends-on": [task("DOC-FE-001", depends_on=_DELETE)],
        "divergent-missing-parent": [task("DOC-FE-001", parent=_DELETE)],
    }
    for name, payload in divergences.items():
        path = OUT / f"{name}.json"
        path.write_text(
            json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        print(f"wrote {path.relative_to(HERE.parent)}  (known divergence)")


if __name__ == "__main__":
    main()
