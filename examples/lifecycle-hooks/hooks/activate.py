#!/usr/bin/env -S python3 -u
"""on_activate — stand the build history up.

This is the work you would otherwise do by hand, from memory, every time
you made a session: create the database, migrate it, backfill it with
something worth querying. It runs once, before the session is
attachable, and if it fails the session does not come up.

The shebang is the point of this file being Python at all. Hook bodies
run under POSIX `sh` unless the first line says otherwise; this one
says otherwise, and `env -S` is the case worth demonstrating — the
kernel hands an interpreter everything after its path as a SINGLE
argument, so `-S python3 -u` arrives at `env` whole and `env` splits it
itself. A shebang parser that split per-word would hand `env` a bare
`-S` with only `python3` attached, and this would not run.

`-u` is not decoration either: stdout here is a pipe to the daemon log,
so without it Python would block-buffer and the ordering of these lines
against anything else the launch logs would be luck.
"""

import os
import sqlite3
import sys
from pathlib import Path

STATE = Path(os.environ.get("HOME", "/home")) / ".local/state/buildlog"
DB = STATE / "builds.db"


SCHEMA = """
CREATE TABLE IF NOT EXISTS builds (
    id         INTEGER PRIMARY KEY,
    suite      TEXT    NOT NULL,
    branch     TEXT    NOT NULL,
    status     TEXT    NOT NULL CHECK (status IN ('passed', 'failed')),
    duration_s INTEGER NOT NULL,
    started_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS builds_by_suite ON builds (suite);
CREATE INDEX IF NOT EXISTS builds_by_time  ON builds (started_at);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"""

SUITES = {0: "lint", 1: "unit", 2: "integration", 3: "e2e"}
BRANCHES = {0: "main", 1: "feat/lifecycle-hooks", 2: "fix/flaky-attach"}

# Lumpy on purpose: (4, 6, 4, 11, 6, 13, 4) builds per day, so the
# sparkline on the attach banner has a shape rather than a flat line.
DAY_BUCKETS = ((4, 6), (10, 5), (14, 4), (25, 3), (31, 2), (44, 1))

BUILDS = 48


def day_offset(n):
    """Which day, counting back from today, build `n` belongs to."""
    for limit, day in DAY_BUCKETS:
        if n <= limit:
            return day
    return 0


def backfill(now):
    """A week of CI history, derived from the row number.

    Deliberately not `random()`: the figures on the banner are the same
    on every machine and across every run, and a demo whose numbers
    drift under you is one people stop trusting.
    """
    for n in range(1, BUILDS + 1):
        kind = n % 4
        duration = {
            0: 12 + (n * 31) % 9,
            1: 48 + (n * 17) % 25,
            2: 240 + (n * 53) % 90,
            3: 165 + (n * 41) % 70,
        }[kind]
        # The intraday offset stays under a day, so a build never lands
        # in the neighbouring day's sparkline column.
        started = now - day_offset(n) * 86400 - (n * 3607) % 86400
        yield (
            SUITES[kind],
            BRANCHES[n % 3],
            "failed" if (n * 7919) % 100 < 15 else "passed",
            duration,
            started,
        )


def main():
    STATE.mkdir(parents=True, exist_ok=True)

    with sqlite3.connect(DB) as db:
        db.executescript(SCHEMA)
        db.execute("PRAGMA journal_mode = WAL")

        now = int(db.execute("SELECT strftime('%s', 'now')").fetchone()[0])
        db.executemany(
            "INSERT OR IGNORE INTO meta (key, value) VALUES (?, ?)",
            [("schema_version", "1"), ("created_at", str(now)), ("attaches", "0")],
        )

        # Idempotent. on_activate fires once per session, so strictly it
        # needn't be — but a setup script that only works against an
        # empty disk breaks the first time someone reruns it by hand,
        # which is exactly when they are already debugging something.
        if db.execute("SELECT count(*) FROM builds").fetchone()[0] == 0:
            db.executemany(
                """INSERT INTO builds
                       (suite, branch, status, duration_s, started_at)
                   VALUES (?, ?, ?, ?, ?)""",
                backfill(now),
            )

        total = db.execute("SELECT count(*) FROM builds").fetchone()[0]

    # Nothing is attached yet, so this goes to the daemon log rather
    # than a terminal. `min session logs` will show it; the attach
    # banner is where a human actually sees the result.
    print(f"buildlog: schema v1 ready at {DB} ({total} builds)")


if __name__ == "__main__":
    try:
        main()
    except sqlite3.Error as e:
        # A failing on_activate fails the activation, and this text is
        # what the user sees attached to that error — so make it the
        # reason, not a traceback.
        print(f"buildlog: could not prepare {DB}: {e}", file=sys.stderr)
        sys.exit(1)
