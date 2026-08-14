# buildlog — lifecycle hooks doing real work

A project whose [lifecycle hooks](../../docs/reference/loadouts.md#lifecycle_hooks---scripts-at-session-transition-points)
stand up a real development environment: a SQLite build history,
migrated and backfilled on activation, and reported on at every attach.

Nothing here is a test fixture. Each hook does the kind of work you
would otherwise do by hand, from memory, every time you made a session.

## Run it

From the repo root:

```sh
just min session activate examples/lifecycle-hooks --attach
```

The first activation **prompts once**, for the project as a whole:

```
  project /…/examples/lifecycle-hooks declares lifecycle hooks
  › Allow once  Allow permanent  Ignore once  …
```

Choose *Allow once*. That prompt is the point — a project's hooks are
arbitrary code from someone else, and a project matching no policy rule
is undecided rather than allowed. Hooks in your own loadouts are not
gated; those are your files already.

## What you should see

**On attach**, the banner:

```
  buildlog  ·  schema v1  ·  up 6s  ·  attach #1
  ───────────────────────────────────────────────────────
  builds       48 (7 failed)
  pass rate    85% passing

  slowest suites (mean wall time)
  integration  ████████████████████  282s
  e2e          ██████████████░░░░░░  198s
  unit         ████░░░░░░░░░░░░░░░░  59s

  volume       ▃▄▃▆▄█▃  builds/day, last 7 (peak 13)

  try:  sqlite3 $BUILDLOG_DB 'select suite, status, duration_s from builds limit 5;'
```

In a colour terminal the name, bars, and sparkline are cyan, and the
pass rate is traffic-lit — green at 90%+, yellow above 75%, red below.
`TERM=dumb` or an unset `TERM` gets exactly the plain text above.

Every figure is a query, and the backfill is derived from the row
number rather than `random()`, so those numbers are the same on your
machine as they are here. A demo whose figures drift under you is a
demo people stop trusting.

**Record a build**, so the next attach has news:

```sh
sqlite3 "$BUILDLOG_DB" \
    "insert into builds (suite, branch, status, duration_s, started_at)
     values ('e2e', 'main', 'failed', 402, strftime('%s','now'));"
```

**Leave and come back.** Type `exit`, choose *keep the session alive*,
then re-attach:

```sh
just min session attach <name>
```

The banner reads `attach #2`, 49 builds, 8 failed, 83% passing, and a
longer `e2e` bar (214s, up from 198s). The database persisted across the
disconnect and the attach hook re-ran against it.

The sparkline will not budge for one build — it quantizes to eight
levels against the week's peak, so today's column needs a few more
before it climbs a step. Insert a handful in a loop if you want to
watch it move.

**Then throw it away:**

```sh
just min session destroy <name>
```

## Why only two transitions

`on_detach` and `on_destroy` are both declarable; this demo skips them
because neither can do anything useful *here*, and a hook that fires
into the void teaches the wrong lesson.

Both are headless, so neither can print to you. Worse for destroy:
nothing it writes survives at all. The session's workspace, home, and
`/state` all live under `sessions/<id>/` on the daemon and are deleted
moments later, and project sync is a one-way upload with no path back to
the host. Tidying up *inside* the sandbox is wasted work — the sandbox
is about to be deleted for you.

What they are genuinely for is reaching **outside** the sandbox, which
is the one thing that outlives it: releasing a preview deployment, a
cloud database, a DNS record, a license seat.

```sh
curl -fsS -X DELETE "$PREVIEW_API/envs/$MINIMAL_SESSION_ID"
```

That needs infrastructure this demo would have to invent. Note that a
failure there is logged and teardown continues regardless — a session
must always be destroyable, so a destroy hook can never wedge one.

## Where the output goes

This is the part that surprises people, and the demo is arranged to make
it obvious rather than to hide it.

| Transition | Output lands in |
|---|---|
| `on_attach` | **your terminal** — it runs on the pty you are attached to |
| `on_activate` | the **daemon log** — nothing is attached yet |

That asymmetry is why the setup work happens in `on_activate` but is
*reported* by `on_attach`: the hook that does the work has no one to
tell, so the hook with a terminal reads the result back out of the
database and draws it.

`min session activate` does report activation hooks by name as they
succeed, so a hook that ran is never entirely invisible:

```
Ran activation hook from project `…/examples/lifecycle-hooks`: build history: migrate, backfill, and report
```

## What's in here

```
minimal.toml         the project: two packages, one var, and its two hooks
hooks/activate.py    migrate + backfill a week of CI history
hooks/attach.py      the status banner (the only hook with a terminal)
```

Both hooks are **external scripts**, resolved against this directory and
carried into the session with the project tree. Hooks can also be
declared inline with `type = "inline"`, which is the usual choice for
one-liners.

Both are also **Python**, which is the other thing this demo is for.

## Things worth trying

**Skip the hooks entirely.** Add `--no-hooks` and re-activate. No
prompt, no database, and the attach banner says so instead of spraying
sqlite errors — a hook is not a guarantee that a previous hook ran.

**See what a session will run, before it runs.**

```sh
just min session hooks <name>
```

Lists every composed hook with the loadout or project that declared it.
It reads the daemon's stored composition, so it works after a daemon
restart and without an attached session.

**Break the activation on purpose.** Add `raise SystemExit(3)` after the
imports in `hooks/activate.py` and re-activate. The session never becomes
attachable and the error carries the hook's output — a failing
`on_activate` is the one place a hook changes control flow. `on_attach`
never blocks its transition; a failure there warns and the attach
proceeds.

**Skip the prompt next time.** Choose *Allow permanent* instead, and the
project's path lands in `[hooks] allow` in your
[`user_policy.toml`](../../docs/reference/user-policy.md).

## The shebang

Hook bodies run under POSIX `sh` unless their first line says
otherwise. Both hooks here say otherwise:

```python
#!/usr/bin/env -S python3 -u
```

That default is deliberate — a hook written to plain `sh` keeps working
in a session whose shell is leaner. But `sh` is a poor language for
drawing a chart, so these hooks opt out, and the banner is the better
for it: `"█" * filled + "░" * (width - filled)` instead of a `while`
loop, `BLOCKS[level]` instead of an eight-arm `case`.

`env -S` is the form worth demonstrating, not just the shortest one
that works. The kernel hands an interpreter everything after its path
as a **single** argument, so `env` receives `-S python3 -u` whole and
splits it itself. A shebang parser that split per-word would hand `env`
a bare `-S` with only `python3` attached, and neither hook would run.
Minimal parses shebangs itself — the scripts arrive on the
interpreter's stdin, so no kernel ever sees that line — and it parses
them the way `execve(2)` does.

`-u` earns its place too. Python block-buffers stdout when it is not a
terminal, so without it the activate hook's line reaches the daemon log
out of order, and the banner can arrive after the shell prompt has
been drawn over it.

Two consequences of minimal reading the shebang rather than the kernel:

- **The interpreter must accept a program on stdin.** Every shell and
  scripting language in common use does. One that insists on a file
  argument (`awk -f`) cannot be driven this way.
- **Name it absolutely, or let `env` find it.** A relative path would
  resolve against the *daemon's* working directory, not the session's.

## stdin is spoken for

The script itself arrives on the interpreter's standard input. In
Python that means `sys.stdin` is already at EOF, so `input()` raises
`EOFError` — a hook cannot prompt you.

That is a feature, not a limitation: it is also what stops a hook
reading your keystrokes out of the terminal you are attached to.
