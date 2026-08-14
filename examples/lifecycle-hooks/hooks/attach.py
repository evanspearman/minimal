#!/usr/bin/env -S python3 -u
"""on_attach — the status banner.

The only transition with a terminal to write to. It runs on the same pty
as your shell, just after the session binds, so whatever it prints lands
in front of you before your first prompt. Everything on_activate did was
headless; this is where it becomes visible.

`-u` matters more here than in activate: this is writing at a terminal a
human is watching, and a block-buffered banner arrives after the shell
prompt has already been drawn over it.

WHY THERE IS A BYTE BUDGET
--------------------------
An attach hook can only write about a pty buffer's worth before it
deadlocks. The daemon awaits this hook from inside the Host's message
loop — the same loop that drains the session pty and forwards it to the
client — so while the hook runs, nothing is reading the pty. Output
lands in a fixed-size buffer, and once that fills the write blocks
forever: the hook wedges until its 60s timeout, and the timeout kills it
before anything it wrote is forwarded. The symptom is not a truncated
banner, it is *total silence*, blamed on whatever the hook happened to
be doing.

Measured: ~700 bytes is fine, 24 kB deadlocks. So this file renders the
whole banner into memory, measures it, and falls back to an uncoloured
version if it is over BUDGET. That makes the deadlock structurally
unreachable rather than a comment nobody reads.

It also rules out animation: repeated frames multiply bytes, and the
hook holds the drain loop anyway, so nothing would flush between them.
One static frame is the only thing that can work here.
"""

import os
import sqlite3
import sys
from pathlib import Path

STATE = Path(os.environ.get("HOME", "/home")) / ".local/state/buildlog"
DB = STATE / "builds.db"

DAYS = 7
BAR_WIDTH = 22
INNER = 68  # visible columns between the frame rails

BLOCKS = "▁▂▃▄▅▆▇█"  # sparkline heights
EIGHTHS = " ▏▎▍▌▋▊▉"  # partial cells, index == eighths of a cell
SHADES = "░▒▓█"  # heatmap density

BUDGET = 4096
DOCS = "https://github.com/gominimal/minimal/blob/main/docs/reference/loadouts.md"

GREEN = (26, 127, 55)
AMBER = (154, 103, 0)
RED = (207, 34, 46)
GREY = (88, 96, 105)


def palette():
    """ANSI codes, or empty strings where colour would be noise.

    This runs on a real pty, so the usual `isatty` check is not the
    question — whether the terminal can render it is. TERM=dumb and an
    unset TERM both fall back to plain text.
    """
    term = os.environ.get("TERM", "")
    if not term or term == "dumb":
        return dict.fromkeys(("bold", "dim", "off"), ""), False
    return {"bold": "\033[1m", "dim": "\033[2m", "off": "\033[0m"}, True


def rgb(colour):
    r, g, b = colour
    return f"\033[38;2;{r};{g};{b}m"


def ramp(t):
    """Green → amber → red across 0..1."""
    if t < 0.5:
        start, end, u = GREEN, AMBER, t * 2
    else:
        start, end, u = AMBER, RED, (t - 0.5) * 2
    return tuple(round(a + (b - a) * u) for a, b in zip(start, end))


def health(rate):
    return GREEN if rate >= 90 else AMBER if rate >= 75 else RED


def bar(value, peak, colour, c):
    """A gradient bar with eighth-of-a-cell precision.

    Eighth blocks give 8x the horizontal resolution of whole cells, so
    two suites 20s apart no longer draw the same bar. The `░` track is
    kept because a bar that vanishes at low values reads as missing
    data rather than as a small number.
    """
    eighths = max(1, round(value * BAR_WIDTH * 8 / peak)) if peak else 1
    eighths = min(eighths, BAR_WIDTH * 8)
    full, rem = divmod(eighths, 8)
    cells = ["█"] * full + ([EIGHTHS[rem]] if rem else [])
    track = BAR_WIDTH - len(cells)
    plain = "".join(cells) + "░" * track

    if not colour:
        return plain, plain
    painted = "".join(
        f"{rgb(ramp(i / max(1, BAR_WIDTH - 1)))}{ch}" for i, ch in enumerate(cells)
    )
    return plain, painted + c["dim"] + "░" * track + c["off"]


def spark(values):
    peak = max(values) or 1
    return "".join(BLOCKS[min(7, v * 7 // peak)] for v in values)


def heatmap(totals, passes, colour, c):
    """One two-cell column per day: density by volume, hue by pass rate.

    Two dimensions in fourteen characters — more than the sparkline it
    replaces, for about the same money.
    """
    peak = max(totals) or 1
    plain, painted = [], []
    for total, passed in zip(totals, passes):
        shade = SHADES[min(3, total * 4 // (peak + 1))] if total else "·"
        plain.append(shade * 2)
        if colour:
            hue = health(round(100 * passed / total)) if total else GREY
            painted.append(f"{rgb(hue)}{shade * 2}")
    if not colour:
        return "".join(plain), "".join(plain)
    return "".join(plain), "".join(painted) + c["off"]


def render(data, colour, c):
    """Build the banner as (plain, painted) pairs.

    Every line is carried in both forms so the frame can be padded from
    the *visible* width. Measuring the painted string would count escape
    bytes as columns and tear the right rail apart.
    """
    rows = []

    def row(plain="", painted=None):
        rows.append((plain, plain if painted is None else painted))

    rate = data["rate"]
    rate_colour = rgb(health(rate)) if colour else ""

    row()
    row(
        f"builds       {data['builds']} ({data['failed']} failed)",
        f"{c['dim']}builds{c['off']}       {data['builds']} "
        f"({data['failed']} failed)",
    )
    row(
        f"pass rate    {rate}% passing",
        f"{c['dim']}pass rate{c['off']}    {rate_colour}{rate}%{c['off']} passing",
    )
    row()

    # Spaced to the data rows below, not to itself: suite(12) + bar +
    # the `{mean:>5}s` column + the trend.
    head = f"{'slowest suites':<12} {'':{BAR_WIDTH}} {'mean':>6}  7-day trend"
    row(head, f"{c['bold']}{head}{c['off']}")

    peak = max(mean for _, mean, _ in data["suites"])
    for suite, mean, trend in data["suites"]:
        drawn, painted_bar = bar(mean, peak, colour, c)
        line = f"{suite:<12} {drawn} {mean:>5}s  {spark(trend)}"
        row(
            line,
            f"{suite:<12} {painted_bar} {mean:>5}s  "
            f"{c['dim']}{spark(trend)}{c['off']}",
        )
    row()

    grid, painted_grid = heatmap(data["totals"], data["passes"], colour, c)
    tail = f"builds/day, last {DAYS} (peak {max(data['totals'])})"
    row(
        f"volume       {grid}  {tail}",
        f"{c['dim']}volume{c['off']}       {painted_grid}  {c['dim']}{tail}{c['off']}",
    )
    row()
    hint = "try:  sqlite3 $BUILDLOG_DB 'select * from builds limit 5;'"
    row(hint, f"{c['dim']}{hint}{c['off']}")

    # OSC 8 hyperlink on the title — the plain form stays "buildlog", so
    # the frame's width arithmetic never sees the escape.
    title = "buildlog"
    painted_title = (
        f"{c['bold']}\033]8;;{DOCS}\033\\{title}\033]8;;\033\\{c['off']}"
        if colour
        else title
    )
    meta = f"schema v{data['version']} · up {data['uptime']} · attach #{data['attaches']}"

    out = []
    lead = f"─ {title} "
    trail = f" {meta} ─"
    fill = INNER - len(lead) - len(trail)
    out.append(
        f"{c['dim']}╭{c['off']}─ {painted_title} "
        f"{c['dim']}{'─' * fill}{c['off']} "
        f"{c['dim']}{meta} ─╮{c['off']}"
        if colour
        else f"╭{lead}{'─' * fill}{trail}╮"
    )
    for plain, painted in rows:
        pad = " " * max(0, INNER - 2 - len(plain))
        out.append(f"{c['dim']}│{c['off']}  {painted}{pad}{c['dim']}│{c['off']}")
    out.append(f"{c['dim']}╰{'─' * INNER}╯{c['off']}")
    # Leading blank line: the banner lands straight after whatever the
    # attach printed, and a box butted up against it reads as debris.
    return "\n" + "\n".join(out) + "\n"


def collect(db):
    """One pass over the table; everything else is arithmetic.

    48 rows is nothing, and bucketing them here rather than issuing a
    query per suite per day is exactly the kind of thing the shebang
    bought us.
    """

    def one(sql):
        return db.execute(sql).fetchone()[0]

    rows = db.execute(
        "SELECT suite, status, duration_s,"
        " (strftime('%s','now') - started_at) / 86400 FROM builds"
    ).fetchall()

    totals = [0] * DAYS
    passes = [0] * DAYS
    by_suite = {}
    for suite, status, seconds, age in rows:
        if 0 <= age < DAYS:
            totals[DAYS - 1 - age] += 1
            passes[DAYS - 1 - age] += status == "passed"
        durations, daily = by_suite.setdefault(suite, ([], [[] for _ in range(DAYS)]))
        durations.append(seconds)
        if 0 <= age < DAYS:
            daily[DAYS - 1 - age].append(seconds)

    suites = sorted(
        (
            (
                suite,
                round(sum(durations) / len(durations)),
                [round(sum(d) / len(d)) if d else 0 for d in daily],
            )
            for suite, (durations, daily) in by_suite.items()
        ),
        key=lambda s: s[1],
        reverse=True,
    )[:3]

    age = one(
        "SELECT strftime('%s','now') - CAST(value AS INTEGER)"
        " FROM meta WHERE key = 'created_at'"
    )
    if age < 60:
        uptime = f"{age}s"
    elif age < 3600:
        uptime = f"{age // 60}m {age % 60}s"
    else:
        uptime = f"{age // 3600}h {age % 3600 // 60}m"

    failed = sum(1 for _, status, _, _ in rows if status == "failed")
    return {
        "builds": len(rows),
        "failed": failed,
        "rate": round(100 * (len(rows) - failed) / len(rows)),
        "uptime": uptime,
        "attaches": one("SELECT value FROM meta WHERE key = 'attaches'"),
        "version": one("SELECT value FROM meta WHERE key = 'schema_version'"),
        "suites": suites,
        "totals": totals,
        "passes": passes,
    }


def main():
    # A hook is not a guarantee that a previous hook succeeded. If
    # activate was skipped (--no-hooks) or failed, say so plainly
    # instead of spraying sqlite errors across the banner.
    if not DB.exists():
        print("buildlog: no database — was this session activated with --no-hooks?")
        return

    c, colour = palette()

    with sqlite3.connect(DB) as db:
        db.execute(
            "UPDATE meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'attaches'"
        )
        if not db.execute("SELECT count(*) FROM builds").fetchone()[0]:
            print("buildlog: database is empty — nothing to report yet")
            return
        data = collect(db)

    banner = render(data, colour, c)
    if len(banner.encode()) > BUDGET:
        # Per-cell colour is what makes this big, so drop it rather than
        # the information. See the module docstring: going over budget
        # does not degrade, it deadlocks.
        banner = render(data, False, dict.fromkeys(c, ""))
    sys.stdout.write(banner)


if __name__ == "__main__":
    try:
        main()
    except sqlite3.Error as e:
        # A failing attach hook warns and the attach proceeds — so this
        # must not look like the session is broken. It isn't.
        print(f"buildlog: banner unavailable ({e})", file=sys.stderr)
