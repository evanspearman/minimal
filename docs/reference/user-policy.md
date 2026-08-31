---
title: User policy
description: "The user_policy.toml reference: the vars/patches allow-deny-ignore schema, where the file lives, how min gates loadout and project contributions against it, and how interactive prompts write rules back."
---

# User policy

Your **user policy** constrains which environment variables and file patches
may enter a session. When [`min session activate`](./cli-min.md#session-activate)
composes a session, it gathers the variables and patches contributed by your
[loadouts](./loadouts.md) and the project's
[`minimal.toml`](./minimal-dot-toml.md) and evaluates each against
`user_policy.toml` before the session finalizes. The policy records these
decisions once — for example, that variables named `AWS_*` are always rejected,
or that patches may originate only under `~/.config` — rather than requiring a
choice on every activation. The allow list does not apply to values specified in your
loadouts, only the ignore and deny lists do.

The policy is enforced **only on the client**, inside `min`, never on the
daemon (see [Sessions](../concepts/sessions.md)). It gates three domains:

- **Variables** — matched by variable **name**.
- **Patches** — matched by the **source file paths** a patch enumerates on
  the host (the patch's `dest` is never matched).
- **Lifecycle hooks** — matched by the **project root path** that declared
  them.

The first two exist to stop a project pulling *your data* into a session. The
third exists for a different reason: a [lifecycle
hook](./loadouts.md#lifecycle_hooks---scripts-at-session-transition-points) is
arbitrary code the project asks to run inside your session, so the risk is
execution rather than disclosure. A project must be allow-listed before any
hook it declares will run. Hooks declared in your own loadouts are not gated —
they are your files already.

Packages are effectively out of scope. A package cannot supply file
patches, nor environment variables that carry host data (values inherited from
your shell), nor lifecycle hooks; because packages cannot transfer host data
into a session or ask to execute code, the policy's protective purpose does not
apply to them.

## Where the policy lives

A single TOML file, beside your client [`config.toml`](./loadouts.md#client-config):

```
<config>/minimal/user_policy.toml
```

`<config>` is the platform user config directory: `$XDG_CONFIG_HOME` on Linux
(or `$HOME/.config` when unset); macOS also uses `$HOME/.config`. The global
[`--config-dir`](./cli-min.md#global-flags) flag overrides the base, and
`min dirs` prints the resolved config directory.

The file is optional. A missing file is treated as an empty policy — a fresh
install activates fine without it. Only `NotFound` is silenced: a file that
exists but can't be read (permissions) or doesn't parse (bad TOML, an invalid
glob) fails the activation loudly.

## Example

```toml
[vars]
allow  = ["MY_APP_*", "RUST_*"]
deny   = ["AWS_*", "*_TOKEN"]
ignore = ["_*"]

[patches]
allow  = ["~/.config/**", "/etc/xdg/**"]
deny   = ["~/.ssh/**", "**/*.pem"]
ignore = ["**/.DS_Store"]
```

## Schema

The file has three optional sections, `[vars]`, `[patches]`, and `[hooks]`.
Each holds three optional keys — `allow`, `deny`, and `ignore` — and each key
is a list of glob patterns. Every key defaults to empty, so any section or key
may be omitted; an empty file is a valid empty policy.

Each of the nine lists accepts either a single bare string or a list of
strings:

```toml
[vars]
deny = "AWS_*"            # single pattern
[patches]
deny = ["~/.ssh/**", "**/*.pem"]   # list of patterns
```

### `[vars]` — Environment-variable rules

Patterns match against variable **names** and are standard globs (`*`, `?`,
`[...]`, `**`). They are compiled when the file loads, so a malformed glob
fails activation immediately with an `invalid glob` error naming the pattern.

| Key | Matches names that… |
|-----|---------------------|
| `allow` | non-user sources (the project) may set. Loadout-origin vars auto-pass this step — see [Origin semantics](#origin-semantics) |
| `deny` | no source may set, regardless of origin. An unconditional reject |
| `ignore` | are silently dropped without prompting, regardless of origin |

```toml
[vars]
allow  = ["MY_APP_*", "RUST_*"]
deny   = ["AWS_*", "*_TOKEN"]
ignore = ["_*"]
```

### `[patches]` — File-patch rules

Patterns match against the **source file paths** a patch enumerates on the
host, checked per file after the patch's glob is walked. A patch's `dest`
inside the session is never matched.

| Key | Matches source paths that… |
|-----|----------------------------|
| `allow` | the project may read from. Loadout-origin patches auto-pass this step |
| `deny` | no patch may read from, regardless of origin |
| `ignore` | are silently dropped without prompting, regardless of origin |

Only the project and your loadouts can contribute patches; packages cannot, so
in practice this section arbitrates project patches against your loadouts'.

Patterns are path globs and may use two forms of expansion, resolved against
the session's already-resolved variables at activation:

- A leading `~/` expands to the host home directory.
- `$NAME` / `${NAME}` references (including `$HOME`) resolve against the
  session vars; referencing an undefined name fails the activation.
- `$LOADOUT_ROOT` is **not** available: a policy is yours, not any one
  loadout's, so there is no loadout for it to name. Referencing it fails the
  activation. Write the path out, or match with a glob.

Unlike loadout patch **sources**, a policy pattern need not be absolute — it is
a matcher, not a walk seed, so a bare `**/*.pem` matches any `.pem` path at any
depth. Patch patterns are held verbatim and compiled only at expansion time
(after variable substitution), so a glob that is invalid on its own but valid
once a `$VAR` is substituted is not rejected up front.

A pattern with **no glob metacharacters covers that path and everything
beneath it**, so `deny = ["~/.ssh"]` and `deny = ["~/.ssh/**"]` are
equivalent. The subtree test is path-component aware: `~/.ssh` does not reach
`~/.sshfs`. Naming a single file is unaffected — a file has nothing beneath
it. This applies to `[patches]` only, because its patterns are matched
against individual files; `[hooks]` patterns match project roots and stay
exact (see below).

> **Behaviour change.** These patterns are matched against files, so before
> this rule existed a bare directory entry matched *nothing at all* — it was
> inert, whichever list it sat in. Two consequences when upgrading:
>
> - A `deny` or `ignore` entry naming a directory starts working. If you also
>   patch a file under that directory, a composition that used to succeed now
>   fails (`deny`) or silently drops the file (`ignore`) — which is what the
>   entry always asked for.
> - An `allow` entry naming a directory starts granting. Project- and
>   package-sourced patches beneath it used to reach the approval prompt and
>   are now allowed without one, so a prompt you were used to seeing may stop
>   appearing.
>
> Both directions deliver what the entry says. Read your `[patches]` section
> once after upgrading if you want to confirm it still says what you meant.

Both the resolved target path and, when a patch source traverses a symlink,
the link path are checked independently; the most restrictive outcome wins.

```toml
[patches]
allow  = ["~/.config/**", "/etc/xdg/**"]
deny   = ["~/.ssh/**", "**/*.pem"]
ignore = ["**/.DS_Store"]
```

### `[hooks]` — Lifecycle-script rules

Patterns match the **project root path** that declared the hook — as you refer
to that project on your own machine, not the daemon's copy of it. The script's
own contents and any file it names are never matched: this section decides
*whose* code may run, not which code.

| Key | Matches project roots whose… |
|-----|------------------------------|
| `allow` | hooks may run |
| `deny` | hooks may never run, and whose presence fails the activation |
| `ignore` | hooks are silently dropped without prompting |

Only the project is arbitrated here. Your loadouts' hooks are your own files
and run without consulting this section; packages cannot declare hooks at all,
and any that appear are denied outright.

Patterns expand the same way patch patterns do (`~/`, `$NAME`, `${NAME}`),
but they do **not** get the patch section's subtree rule: a pattern here
names one project and matches it exactly. `allow = ["~/work/trusted"]`
allows that project and not a project nested inside it — one approval is one
project, and a hook is arbitrary code. Write `~/work/**` when you do mean
every project under a directory. They are globs, so a path containing glob
metacharacters must be escaped to match itself — the prompt does that for you
when you choose a permanent rule, which is why a hand-written entry is best
kept to a plain path.

A project matching nothing in this section is **undecided**, not allowed: it
reaches the prompt, and under `--no-prompt` it fails the activation with a
snippet naming the project. Silence is never consent — a hook is arbitrary
code from someone else.

```toml
[hooks]
allow = ["~/work/**"]
deny  = ["/tmp/**"]
```

## How a contribution is decided

Every variable, every patch source, and every project that declares lifecycle
hooks is categorized against the relevant section in a fixed precedence:

1. **`deny`** — if it matches, the composition fails. Deny takes precedence
   over every other rule, including `ignore`: an item matched by both `deny`
   and `ignore` resolves as **denied**, so a would-be rejection cannot be
   masked by an ignore glob.
2. **`ignore`** — if it matches, the item is silently dropped from the session
   (no prompt, no failure).
3. **allow** — origin-aware, described next.

### Origin semantics

Every item carries the source that contributed it, and the allow step depends
on it:

- **User-origin** (items from your own [loadouts](./loadouts.md)) **auto-pass**
  the allow step — you don't have to allow-list what you declared yourself.
  They are still subject to `deny` and `ignore`.
- **Non-user-origin** items must match an `allow` pattern to pass cleanly. If
  none matches, the item is **undecided** and routes to an interactive prompt
  (or aborts under `--no-prompt`; see below). In practice the non-user
  contributor is the **project**: it can contribute both gated patches and
  gated variables. A package can only reach this step with a static-valued
  variable — its patches and host-inherited variables are dropped before the
  gate.

Composing several loadouts does not compose policy — the policy is a single
file about what you let *other* sources contribute, kept separate from what
your loadouts themselves contribute.

## Prompts and writing rules back

When the policy can't decide a non-user-origin item (no `allow`, `deny`, or
`ignore` match), `min` prompts interactively during activation. Each prompt
offers six choices:

| Choice | Effect |
|--------|--------|
| Allow once | Accept for this activation only |
| Allow permanent | Accept and append the name/path to `[…].allow` |
| Ignore once | Drop for this activation only |
| Ignore permanent | Drop and append the name/path to `[…].ignore` |
| Abort activation | Halt, recording nothing |
| Deny permanent | Halt and append the item to `[…].deny` so future activations reject it before prompting |

The three **permanent** choices edit `user_policy.toml` in place. `min` writes
the updated file atomically (via a `.tmp` sibling) after backing up the
previous contents to `user_policy.toml.bak`, then prints `Updated <path>`. If
the file (or its directory) isn't writable, the permanent choices are hidden
and only the once/abort actions are offered; a failed save is reported as a
warning and does not, on its own, fail the activation.

### Non-interactive activation

Under [`--no-prompt`](./cli-min.md#global-flags), or when stdin/stderr is not a
TTY (CI, pipes, agents), `min` never prompts. If any item would have required a
decision, the activation aborts before contacting the daemon and prints a
ready-to-paste `user_policy.toml` snippet listing what to add. Add the rules
and re-run. When the policy already decides every item, a non-interactive
activation proceeds normally.

## Interactions and notes

- **Client-only enforcement.** The daemon never sees or runs your policy. `min`
  resolves and gates contributions before (and, for daemon-surfaced items,
  during) the activation round-trip; a `deny` match fails on the client.
- **Captured at activation.** The policy is read once per activation. Editing
  it does not change sessions that already exist — destroy and re-activate to
  pick up an edit.
- **Loadout conflicts.** When two contributors set the same variable name to
  different values, composition fails; the hint is to add that name to your
  policy's `ignore` list to drop all contributors of it. See
  [Composition, conflicts, and policy](./loadouts.md#composition-conflicts-and-policy).
- **Diagnostics.** `min` support bundles include a **redacted**
  `config/user_policy.toml.redacted` copy of the file.
- Only the `min` CLI consumes `user_policy.toml`; `mip`, `minimald`, and
  `minvmd` do not.
