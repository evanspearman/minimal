---
id: UPOL
title: Cedar user policy
owner: evanspearman
epic: gominimal/inbox#1231
arch: https://github.com/gominimal/arch/blob/327e1b57d16b31ae1f4249cb317e3bd5b22e98b8/architecture.md
arch_sha: "327e1b57d16b31ae1f4249cb317e3bd5b22e98b8"
updated: 2026-11-16
---

# UPOL — Cedar user policy

## Context

The user policy decides what a box can take from the user's machine and what it can run. Today it is a TOML file of global allow, deny and ignore lists. A rule that allows a sensitive file for one trusted project allows it for every project, with no prompt. A hook approval matches a directory only, so a `git pull` that brings in new hook scripts keeps the approval.

After this ships, the user policy is a Cedar file that `min` reads on the client. A rule names a principal (a project, a loadout, or a package from an upstream other than gominimal/pkgs), the action it takes, the item it asks for, and optionally the box the request is for. A rule can pin a principal to a hash of its contents, so a change makes `min` ask again. Lifecycle hooks are approved one at a time, by name. A sensitive item enters a box only when every principal in that box can coexist with it. The CLI, its prompt, its headless refusal and `min task run` use the same policy. The daemon applies the client's verdicts. An existing TOML policy is converted on a best-effort basis, and a fresh install gets a default policy.

**Success:** A rule that permits a host variable for one project, or for one box of it, decides nothing for any other project or box. A pinned approval asks again after the principal's files change.

**First slice:** `min` reads `user_policy.cedar` and decides each host variable that a project inherits as a `readHostVariable` request, with the four outcomes, the prompt and the headless refusal. This is the epic's order of work up to its first gate: the file (S1), the outcomes (S2), the principals (S3) and one gate (S6).

## Users and stories

**Roles:** user who controls what boxes take from my machine, user, user writing rules, user who approves a principal once, user who installs packages from upstreams other than gominimal/pkgs, user approving hooks, user with no loadouts configured, user answering a prompt, user asked again after a principal changes, user who meets a loadout or project that my policy does not name, or that changed since I trusted it, user running `min` with `--no-prompt` or with no terminal, user whose box was refused, user whose box is missing a file or a variable I expected, user with an existing `user_policy.toml`, new user, user who runs a project's tasks, developer who reads the box spec to learn what `min` does

- AS A user who controls what boxes take from my machine, I WANT to write my user policy as Cedar rules in one file that `min` validates against a published schema, SO THAT a mistake in a rule is reported when the file loads, not discovered as a silent pass.
  <!-- Acceptance criteria, for the EARS step:
       - `min` reads `user_policy.cedar` from its config directory, `<config>/minimal/`. `min dirs` prints the path. The global `--config-dir` flag changes where `min` looks for the file, as it does for `config.toml`.
       - The schema (principals, actions, resources, their attributes and the request context, per the Vocabulary table and S3) is one file in the source tree, and `min` embeds it at build time. `docs/reference/user-policy.md` links to that file and does not copy it.
       - `HostFile` carries `path`, and `HostVariable` and `BoxVariable` carry `name`, as String attributes equal to the id. A rule such as `forbid(principal, action == Action::"readHostFile", resource) when { resource.path like "*/.DS_Store" };` validates.
       - A file that does not parse, or that does not validate against the schema, makes every activation fail. The error names the file, the line and the Cedar diagnostic.
       - An `@effect` annotation with a value other than `"ignore"` fails the load in the same way, and the error names the value.
       - `user_policy.toml` is no longer read as a policy. S19 covers what happens when it is present.
  -->
- AS A user, I WANT to mark a `forbid` rule as "ignore" so the item is left out and the activation continues, SO THAT I keep the two refusals I have today: quietly drop this, or stop everything.
  <!-- Acceptance criteria, for the EARS step:
       - `min` maps each Cedar answer to one outcome. Allow is Allowed. Deny where every matching `forbid` has `@effect("ignore")` is Ignored. Deny where a matching `forbid` lacks it is Denied. Deny with no matching rule is Undecided, and the user is asked.
       - An ignore rule and a deny rule that both match give Denied.
       - If any policy fails to evaluate for a request, the outcome is Denied, and the error names the policy's `@id` when it has one.
       - Unit tests cover each row of the mapping table in the design.
  -->
- AS A user writing rules, I WANT to name a project or a loadout, test the contents of its definition, and test which of a project's boxes the request is for, in a `when` condition, SO THAT a rule applies to the principal and the box I mean and to no other.
  <!-- Acceptance criteria, for the EARS step:
       - A loadout is `Loadout::"<canonical path of its root directory>"`. A loadout in the flat layout, with `<name>.toml` beside `<name>/` rather than inside it, is addressed by the path of `<name>/`, as if its file were `<name>/loadout.toml`.
       - A project is `Project::"<canonical path of its root>"`. The walk that finds the project root (`crates/minimal/src/cmd/project.rs`) canonicalises the path, so a project reached through a symlink has one id.
       - The project's requests carry everything the box's expanded entry carries, including what `[defaults]` and `[defaults.<type>]` contribute.
       - The schema declares these attributes. A loadout carries `path` (its canonical root as a String), `sha256` (S4), `packages` (a `Set<String>`), `name` and an optional `description`. A project carries `path`, `sha256`, `upstream_url` and an optional `upstream_commit`.
       - The variables a loadout declares are entity tags. The tag name is the variable name. The tag value is the declared literal value, or `""` for an inherited variable. `principal.hasTag("AWS_PROFILE")` is true when the loadout declares `AWS_PROFILE`.
       - Every action declares the same context type, so every request carries `context.box`, for every principal. `context.box.name` is the box's entry name in `minimal.toml`, not the running box's name. `context.box.type` is its Box Type. `context.box.packages` is a `Set<String>` of its expanded entry's packages. `context.box.variables` is a `Set<String>` of the variable names its expanded entry declares. `context.box.spec_digest` is optional and holds the digest of its expanded Box Spec, when the expansion computes one (gominimal/inbox#731 S8).
       - A box created from the implicit `[sessions.default]` entry (spec 29 BOX-165) has `context.box.name == "default"`. That covers a project with no session entry, a project with only a legacy `[session]` table, and a project with no `minimal.toml`.
       - Every gate knows which box a request is for. `crates/minimal/src/cmd/boxes/create.rs` and `crates/minimal/src/task.rs` pass the selected entry to the gates that build the context: `compute_verdict` (`crates/minimal/src/cmd/mod.rs`), `handle_response` (`crates/boxes/src/client/handler.rs`) and `gate_task_env` (`crates/minimal/src/task.rs`). The wire does not carry the entry name, and it does not need to.
       - A test proves three things. A rule with `principal == Project::"/a"` and `when { context.box.name == "dev" }` decides requests for box `dev` of project `/a` and not for box `ci`. The same rule with no `when` decides requests for both. Neither rule decides a request from project `/b`.
  -->
- AS A user who approves a principal once, I WANT a hash of the principal's contents that I can pin a rule to, SO THAT any change to the principal withdraws an approval I pinned.
  <!-- Acceptance criteria, for the EARS step:
       - `min` first computes the git tree object id of the principal's files, the way git computes it, with git's SHA-256 object format in place of SHA-1. Blobs, tree entries, entry order and modes (`100644`, `100755`, `120000`, `40000`) follow git. `min` computes it from the files on disk and needs no git repository.
       - The content hash is SHA-256 over the header `minimal-source-hash-v1\0` followed by the 32-byte binary tree id. A change to how the hash is computed bumps the header version, so every pinned rule asks again, and the change ships as a breaking release.
       - The files are those the design's Scope rule names. Hidden files are included. `.git`, matches of the principal's own `.gitignore` files, and upload exclusions are skipped. Global gitignore, `.git/info/exclude` and `.ignore` files are not read. A symlink is hashed as its target text, as git stores it.
       - For a clean checkout of a SHA-256 git repository with no untracked files, no upload exclusions, no tracked files that match `.gitignore`, no submodules and no clean or smudge filters, the tree id equals `git rev-parse HEAD^{tree}`, and the content hash equals SHA-256 over the header and that id. A test proves it.
       - A loadout in the flat layout is hashed as if `<name>.toml` were `<name>/loadout.toml`, together with any files in `<name>/`. A flat loadout and a directory loadout with the same files give the same hash. A test proves it.
       - A rename, a move, an executable-bit change or a content change each change the hash. Tests cover each.
       - The hash is computed before the first gate that needs it. Activation computes it in the pass that reads files for upload and hook staging. The task path computes the project hash in its own pass, before its gate. An unreadable file fails the activation or the task.
       - `min policy hash <path>` prints the hash of the loadout or project whose root directory is `<path>`. For a loadout, `<path>` is the loadout directory `<name>/`, in both layouts. A path to a `.toml` file is refused, and the message names the directory to pass instead.
  -->
- AS A user, I WANT a rule to decide `readHostFile` for each host file that a patch would copy, separately for each principal, SO THAT approving a file for one principal does not approve it for every principal.
  <!-- Acceptance criteria, for the EARS step:
       - Each host file a patch copies is a `HostFile::"<canonical path>"` request. The principal is the loadout that declares the patch, or the project when the box's expanded entry carries it.
       - When the path is a symlink, the link and its target are two requests: `HostFile::"<absolute path of the link, not resolved>"` and `HostFile::"<canonical path of the target>"`. The file enters only when both are Allowed. If either is Denied, the activation fails. If either is Ignored, the file is left out, and the activation continues. An Undecided request asks the user, as any other request does.
       - Rules match absolute paths. They do not expand `~/` or `$NAME`, so no principal can change what a rule matches.
  -->
- AS A user, I WANT a rule to decide `readHostVariable` for each variable that a principal inherits from my environment, separately for each principal, SO THAT a secret in my environment enters a box only for the principals I chose.
  <!-- Acceptance criteria, for the EARS step:
       - Each inherited variable that takes its value from the host is a `HostVariable::"<name>"` request. The principal is the loadout that declares the variable, or the project when the box's expanded entry carries it. A package cannot inherit a host variable: the daemon strips such variables before gating (`crates/boxes/src/daemon/composer.rs`).
       - An inherited variable that falls back to its default carries no host data and is checked as `setBoxVariable` (S7), not as `readHostVariable`.
       - Outcomes apply as S2 states. An Undecided request asks the user and names the principal and the variable.
  -->
- AS A user, I WANT a rule to decide `setBoxVariable` for each variable a principal defines in my box, SO THAT a principal cannot redefine a variable such as `PATH` or `LD_PRELOAD` without my approval.
  <!-- Acceptance criteria, for the EARS step:
       - Each literal variable, and each inherited variable that falls back to its default, is a `BoxVariable::"<name>"` request. The principal is the loadout or package that declares the variable, or the project when the box's expanded entry carries it.
       - The prompt for an Undecided request shows the value that would be set.
       - A Denied request fails the activation and names the principal, the variable and the rule's `@id`.
       - `min` gates each loadout's variables and patches before it merges the loadouts. An Ignored item never reaches conflict detection, so `@effect("ignore")` on `EDITOR` lets two loadouts that both set it compose, with `EDITOR` left out (gominimal/inbox#763).
  -->
- AS A user, I WANT a host file or host variable to enter a box only when every principal in that box is permitted to coexist with it, SO THAT a principal I trust cannot carry my secret into a box that a principal I do not trust shares.
  <!-- Acceptance criteria, for the EARS step:
       - `min` holds the host values of loadout variables and the host files of loadout patches until the daemon's `ContributionResponse` arrives, which names every package in the box from a non-default upstream (S9a). `min` sends the held items with its verdict, not in `ConfigureLoadout` (`crates/minimal/src/cmd/boxes/create.rs`).
       - For each host file or host variable that a `read` request allowed, `min` evaluates `coexistWithHostFile` or `coexistWithHostVariable` for every principal in the box: the project, each selected loadout, and each package from a non-default upstream.
       - The item enters only when every coexist request is Allowed. A Denied coexist request fails the activation and names the principal and the item. An Undecided one asks the user and names the principal.
       - A test builds a box that is permitted to read `AWS_PROFILE`, with a loadout that has no coexist rule, and shows that the user is asked before the variable enters.
       - A test adds a package from a non-default upstream that has no pending items, and shows that its coexist request is evaluated before a loadout's host variable leaves the client.
  -->
- AS A user who installs packages from upstreams other than gominimal/pkgs, I WANT such a package to be a principal that my rules can name, with the box variables it sets gated, SO THAT a third-party package cannot set variables in my box, or share a box with my secrets, without my approval.
  <!-- Acceptance criteria, for the EARS step:
       - A package whose upstream is not `https://github.com/gominimal/pkgs` is `Package::"<upstream url>#<package name>"`, with `branch` and `locked_commit` as attributes.
       - `ContributionResponse` (`crates/boxes/src/wire/request.rs`) carries every package in the box from a non-default upstream, with its upstream URL, branch and locked commit. Today a package's wire source is only `Package { name }` (`crates/boxes/src/core/source.rs`).
       - Each variable such a package sets is a `setBoxVariable` request (S7).
       - Packages from gominimal/pkgs are not principals and make no requests.
  -->
- AS A user, I WANT every package from an upstream other than gominimal/pkgs to need an `installPackage` decision before it enters a box, SO THAT no project, loadout or package brings third-party code into my box without my approval.
  <!-- Acceptance criteria, for the EARS step:
       - A project whose box lists a package from a non-default upstream in its expanded entry, or a loadout that lists one, makes an `installPackage` request on `Package::"<upstream url>#<package name>"`, with the project or the loadout as principal.
       - A package from a non-default upstream whose dependency also comes from a non-default upstream makes an `installPackage` request for that dependency, with the package as principal.
       - A Denied request fails the activation and names the principal and the package. An Ignored package is left out, and the activation continues.
       - A package from gominimal/pkgs makes no `installPackage` request.
  -->
- AS A user approving hooks, I WANT every lifecycle hook to carry a name, SO THAT a rule can address one hook rather than every hook a principal declares.
  <!-- Acceptance criteria, for the EARS step:
       - A hook declaration requires `name`: `[[lifecycle_hooks]]` in a loadout, and in a project `[[defaults.session.lifecycle_hooks]]`, `[[session.lifecycle_hooks]]` and `lifecycle_hooks` in a `[boxes.<name>]` entry. A hook without it fails to parse with exit 3, and the error names the file and the hook's position. A loadout parse error exits 1 today, so this story maps it to exit 3, as arch's exit-code table states for invalid configuration.
       - Two hooks with the same name in one project or loadout fail to parse, and the error names both.
       - `docs/specs/11-spec-lifecycle-hooks`, `docs/reference/loadouts.md` and `docs/reference/minimal-dot-toml.md` show `name`. The release notes state the breaking change.
  -->
- AS A user, I WANT a rule to decide `executeLifecycleHook` for one named hook and event, or for many hooks at once, SO THAT I can approve one hook without approving the hooks that a principal adds later.
  <!-- Acceptance criteria, for the EARS step:
       - At activation, before any hook runs, `min` makes one `LifecycleHook::"<source>#<hook>:<event>"` request for each event that a hook declares. A hook that runs later, such as `on_attach` or `on_destroy`, runs under the verdict given at activation. The verdict that `min` returns to the daemon carries one entry for each hook and event, which changes `WireHookVerdict` (`crates/boxes/src/wire/policy.rs`). The principal is the loadout that declares the hook, or the project when the box's expanded entry carries it. `<source>` is the canonical path of the project or the loadout that declares the hook, so one rule on a project's hook covers every box that carries it unless the rule tests `context.box`.
       - A hook that is not Allowed does not run. Denied fails the activation. Ignored skips the hook, and the activation continues.
       - The prompt asks once per principal and lists each undecided hook by name and event. The user can approve them all or none. A permanent answer writes one rule for each listed hook, each with an `@id`, the exact principal, action and resource, and a `sha256` pin.
       - A test adds a new hook to an approved principal, and shows that the new hook is Undecided while the approved hook stays Allowed.
  -->
- AS A user, I WANT every loadout gated the same way, with trust stated by a rule, SO THAT where I saved a loadout never decides what it can take.
  <!-- Acceptance criteria, for the EARS step:
       - `min` reads loadouts only from `<config>/minimal/loadouts/`. No loadout skips a gate because of its directory.
       - A rule such as `permit(principal == Loadout::"<path>", action, resource);` permits every request from that loadout. A matching `forbid` still wins.
       - `docs/concepts/loadouts.md` and `docs/reference/user-policy.md` show the trust rule, with and without a pin.
  -->
- AS A user with no loadouts configured, I WANT the orientation banner and the shaped prompt to come with `min` itself, not from a built-in loadout, SO THAT a box with no loadouts makes no user-policy request, also under `--no-prompt`.
  <!-- Acceptance criteria, for the EARS step:
       - `min` no longer composes a built-in `default` loadout (`BUILTIN_DEFAULT_TOML` in `crates/minimal/src/loadouts.rs`).
       - A box created with no loadouts still shows the orientation banner and the shaped prompt. They come from the session shell setup that ships with `min`, and they make no request to the user policy.
       - `MINIMAL_LOADOUTS` and the banner no longer report `default (built-in)`.
       - A loadout that sets `PS1` or `PROMPT_COMMAND` still changes the prompt, and S7 gates it.
       - With no loadouts and an empty policy file, `min` with the global `--no-input` flag creates a box for a project that declares no variables, patches or hooks, and does not fail.
  -->
- AS A user answering a prompt, I WANT "allow permanently", "ignore permanently" or "deny permanently" to write one narrow Cedar rule to my policy, SO THAT one keypress grants exactly the request I saw and nothing wider.
  <!-- Acceptance criteria, for the EARS step:
       - The prompt offers allow once, ignore once, abort, and permanent allow, ignore or deny, as today.
       - A permanent answer appends one rule with `@id("prompt-<date>-<n>")`, an exact principal, action and resource, and `when { principal.sha256 == "<hash>" }` for loadout and project principals. Ignore writes `@effect("ignore")`.
       - For a project's request, a permanent answer also asks for its scope, with "only this box" first. "Only this box" adds `context.box.name == "<entry name>"` to the rule's `when`, so an answer for `dev` does not decide for `ci`. "Every box in this project" adds no box condition. For a loadout or a package, there is no scope question.
       - Appending leaves every other byte of the file unchanged. A test compares the file before and after.
       - Apart from the trust prompt of S16, the prompt offers no rule wider than the request.
  -->
- AS A user asked again after a principal changes, I WANT the prompt to say that the principal changed since I approved it, SO THAT I review the change rather than approve a request that looks new.
  <!-- Acceptance criteria, for the EARS step:
       - When a rule that `min` wrote or printed (an `@id` that starts with `prompt-` or `cli-`) matches the request in all but its `sha256`, the prompt says that the principal changed since the approval and names the rule's `@id`.
       - A request that no such rule nearly matches is presented as a first request.
  -->
- AS A user who meets a loadout or project that my policy does not name, or that changed since I trusted it, I WANT the prompt to offer blanket approval for that loadout, that project, or one box of that project, pinned or not, before it asks about each request, SO THAT a loadout or project I wrote stops prompting after one answer, and I still choose request by request for principals I do not trust.
  <!-- Acceptance criteria, for the EARS step:
       - When a loadout or project makes an undecided request, the prompt first asks about the principal in two cases. In the first, no rule in the policy has `principal == <that principal>` in its scope. In the second, the only such rules are pinned rules that `min` wrote or printed (S15) whose `sha256` no longer matches, and the prompt says that the principal changed since approval (S15).
       - The prompt names the principal. For a project, it names the box the request is for and offers the scope first: every box in this project, or only this box. "Only this box" adds `context.box.name == "<entry name>"` to the rule. For a loadout, there is no scope question. The prompt then offers three choices:
  -->
- AS A user running `min` with `--no-prompt` or with no terminal, I WANT the refusal to print the Cedar rules that would allow each undecided request, SO THAT I can paste them into my policy and run again.
  <!-- Acceptance criteria, for the EARS step:
       - With `--no-prompt`, the global `--no-input` flag, or no terminal, an Undecided request fails the activation. The error prints one ready-to-paste Cedar rule for each undecided request, in the form S14 writes. A rule for a project's request is limited to the box, with `context.box.name`, the narrower of S14's two scopes. Each printed rule has `@id("cli-<UTC date and time to the second>-<4 random hex digits>-<n>")`. The random part is the same for one run, and `<n>` counts the rules that run prints, so no two printed rules share an `@id`. The error names `<config>/minimal/user_policy.cedar`.
       - `min dash` refuses with a one-line error and does not prompt.
       - `crates/minimal/tests/cli.rs` pins the output.
  -->
- AS A user whose box was refused, I WANT the message to name the rule that refused it, SO THAT I can find and change that rule.
  <!-- Acceptance criteria, for the EARS step:
       - A Denied outcome names the `@id` of each matching `forbid` rule. A rule without `@id` is named by its position in the file.
       - This holds for requests from projects, loadouts and packages, at every gate.
  -->
- AS A user whose box is missing a file or a variable I expected, I WANT to see which items the user policy ignored for that box, and the rule that ignored each one, SO THAT I can tell a rule's decision apart from a fault after the activation output is gone.
  <!-- Acceptance criteria, for the EARS step:
       - Each verdict that `min` sends to the daemon carries the `@id` of the deciding rule. The verdict types in `crates/boxes/src/wire/policy.rs` gain the field.
       - Ignored loadout items, which `min` drops today before they reach the wire (`crates/boxes/src/client/composer.rs`), are sent as Ignored verdicts too. An Ignored item is sent without its value.
       - The box record stores each Ignored item with its principal, its resource and the rule's `@id`, for as long as the box record exists.
       - `min box show <box>` shows the Ignored items in a user-policy section, and `-o json` includes them.
       - Spec 29 BOX-159 gains the box record's Ignored items, and spec 28 BCLI-003 renders them. Both amendments land with this story.
  -->
- AS A user with an existing `user_policy.toml`, I WANT a command that writes an equivalent Cedar policy and tells me which rules it could not convert, SO THAT my rules carry across the upgrade wherever Cedar can express them, and I review the rest before anything runs under it.
  <!-- Acceptance criteria, for the EARS step:
       - When `user_policy.toml` exists and `user_policy.cedar` does not, activation refuses. The refusal names `min policy convert`.
       - `min policy convert` writes `user_policy.cedar` from the TOML rules and leaves `user_policy.toml` in place.
       - A glob in a TOML rule becomes a `like` condition on `resource.path` or `resource.name` (S1). The TOML globs use `globset` defaults, where `*` crosses `/` as Cedar's `*` does, so `~/.ssh/**` and `*.pem` convert exactly. `~/` expands to the home directory of the user who runs the command.
       - Each rule it cannot express exactly appears in the output file as a comment with the original rule and the reason. The forms that do not convert exactly are `?`, `[...]`, `{a,b}`, a `/**/` in the middle of a path, and `$NAME`. The command prints how many rules it converted and how many it did not.
       - A `[hooks]` allow rule for an exact project root becomes `permit(principal == Project::"<root>", action == Action::"executeLifecycleHook", resource);`, which covers every box the project declares, as the TOML rule did. A root glob such as `~/work/**` becomes `permit(principal is Project, action == Action::"executeLifecycleHook", resource) when { principal.path like "/home/<user>/work/*" };`, which matches the same project roots under `globset` defaults. The `is Project` keeps it to project hooks, as the TOML `[hooks]` rules were. A deny rule becomes the same rule as a `forbid`, and an ignore rule a `forbid` with `@effect("ignore")`.
       - The converted policy validates against the schema (S1).
  -->
- AS A new user, I WANT `min` to write a default user policy when I have none, SO THAT I start from a policy that keeps well-known secrets out of every box and shows the common rules.
  <!-- Acceptance criteria, for the EARS step:
       - When neither `user_policy.cedar` nor `user_policy.toml` exists, the first activation writes `user_policy.cedar` from a template that ships with `min`.
       - The default policy validates against the schema and contains no `permit` rule.
       - It contains active `forbid` rules on every action for a `HostFile` under `~/.ssh`, `~/.aws` or `~/.gnupg`, with `~` expanded to the user's home directory when the file is written. It contains an `@effect("ignore")` rule for `.DS_Store` files.
       - It contains commented examples of a loadout trust rule, a pinned project rule, and a project rule limited to one box with `context.box.name`.
       - A test activates a box whose loadout patches a file under `~/.ssh` with the default policy, and shows that the activation fails and names the `forbid` rule's `@id`.
  -->
- AS A user who runs a project's tasks, I WANT every way of running a task to gate the task's variables with the same Cedar policy and prompt as any other box, SO THAT a project I have not read cannot take more of my environment through a task than through a session.
  <!-- Acceptance criteria, for the EARS step:
       - `min task run` gates a task's variables. Each variable in a task's `env_vars` that takes its value from the host is a `readHostVariable` request, with the project as principal and the task's box in `context.box`, whose `name` is the task's entry name. `min` decides it before it reads the value. This replaces the TOML gate in `crates/minimal/src/task.rs`.
       - Each literal task variable is a `setBoxVariable` request (S7).
       - A host variable that a read request allowed enters the task only after S8's coexist step. `min` holds the value until it knows every principal in the task's box: the project, each loadout the box carries (tasks pick up `default_loadouts` today, `crates/minimal/src/task.rs`), and each package from a non-default upstream.
       - A rule pinned to the project's `sha256` is evaluated on the task path. `min` computes the project hash before the gate (S4).
       - In-box `min run` refuses a task whose `env_vars` inherit a host value, and the message names `min task run <task>`.
       - The task path uses the S2 outcomes, the S14 prompt and permanent rules, the S16 trust prompt and the S17 headless refusal. A permanent answer writes to `user_policy.cedar`.
       - A test runs a task that inherits `AWS_PROFILE` with the global `--no-input` flag, under a policy with no rule for it, and shows that the task fails and prints the rule that would allow it.
  -->
- AS A user who runs a project's tasks, I WANT the host paths a task mounts and the third-party packages it installs to need a decision from my policy, SO THAT a task cannot mount my files or bring in third-party code that a session would have to ask me about.
  <!-- Acceptance criteria, for the EARS step:
       - Each patch key a task declares with an absolute host path is a request with the project as principal and the task's box in `context.box`. A read-only `dir` or `file` key is a `readHostFile` request on `HostFile::"<declared path>"`. A read-write key is a `readHostFile` request and a `writeHostFile` request, and it needs both Allowed. A `~/` key mounts from the box's home, not the host, and makes no request.
       - `min` sends the daemon a verdict that names each allowed key with its kind and mode, and each dropped key. The daemon mounts only the allowed keys (`crates/minimald/src/exec/process.rs`). It refuses the task when the task declares patches and no verdict came, when the task declares a key that the verdict does not name, or when a key's kind or mode differs from the verdict. This follows the design of the closed gominimal/minimal#2299.
       - In-box `min run` refuses a task that declares patches, and the message names `min task run <task>`.
       - Each package that a task lists from a non-default upstream is an `installPackage` request (S9b), with the project as principal, decided before the daemon installs it. The verdict names the allowed packages, and the daemon installs no other package from a non-default upstream.
       - A test runs a task that declares a read-write patch on a host directory, under a policy that permits `readHostFile` and not `writeHostFile`, and shows that the task fails before the daemon mounts anything.
  -->
- AS A developer who reads the box spec to learn what `min` does, I WANT spec 29 to describe loadout patches and variables as gated by the Cedar user policy before they are read, SO THAT the spec and this epic do not promise two different gates.
  <!-- Acceptance criteria, for the EARS step:
       - BOX-167 and BOX-182 read a loadout's config-file patches under the user policy (`readHostFile`, S5), not under `[loadout-file-rules]`. An Undecided read with no terminal fails the expansion with exit 3, and the error names the file and the request.
       - BOX-183 compares only the variables that the user policy allowed, after the gate (S7). Its remedy names an `@effect("ignore")` rule in `user_policy.cedar`, not the TOML ignore list.
       - `box_entry` in `crates/minimal/src/cmd/boxes/create.rs` decides each inherited variable before it reads the value from the environment, as S6 requires.
  -->

## Requirements

- **UPOL-001** THE SYSTEM SHALL read the user policy from `user_policy.cedar` in the `minimal` directory under the config directory that `min dirs` prints.
  tier:     T0
  verify:   just test-one upol_policy_read_from_config_dir

- **UPOL-002** WHEN the global `--config-dir` flag is given THE SYSTEM SHALL read `user_policy.cedar` from the `minimal` directory under that path.
  tier:     T0
  verify:   just test-one upol_policy_follows_config_dir_flag

- **UPOL-003** THE SYSTEM SHALL validate the user policy against the schema that `min` embeds at build time from one schema file in the source tree.
  tier:     T0
  verify:   just test-one upol_policy_validated_against_embedded_schema

- **UPOL-004** THE SYSTEM SHALL keep a link to the schema file in the user-policy reference page, with no copy of the schema in the page.
  tier:     T0
  verify:   just test-one upol_reference_links_schema

- **UPOL-005** THE SYSTEM SHALL give each `HostFile` a `path` attribute, and each `HostVariable` and `BoxVariable` a `name` attribute, each a String equal to the entity id.
  tier:     T0
  verify:   just test-one upol_resource_string_attributes

- **UPOL-006** IF the user policy file does not parse or does not validate against the schema THEN THE SYSTEM SHALL fail every activation and every `min task run` with exit 3 and a message that names the file, the line and the Cedar diagnostic.
  tier:     T0
  verify:   just test-one upol_invalid_policy_fails_activation

- **UPOL-007** IF a rule carries an `@effect` annotation whose value is not `"ignore"` THEN THE SYSTEM SHALL fail the load with exit 3 and a message that names the file, the line and the value.
  tier:     T0
  verify:   just test-one upol_unknown_effect_fails_load

- **UPOL-008** THE SYSTEM SHALL decide every request from `user_policy.cedar` alone, whether or not `user_policy.toml` exists.
  tier:     T0
  verify:   just test-one upol_toml_policy_not_read

- **UPOL-009** WHEN Cedar answers Allow for a request THE SYSTEM SHALL give the request the outcome Allowed.
  tier:     T2
  verify:   just test-one upol_outcome_allow
  property: for every Cedar answer, Allow maps to Allowed.
  harness:  kani_outcome_mapping_is_total, exhaustive to an unwind bound of 5, which covers every Cedar answer with up to 4 matched rules, each with or without the ignore mark, with and without an evaluation error. Requires the mapping to be a pure function over a summary of Cedar's response, separate from the call into Cedar.

- **UPOL-010** WHEN Cedar answers Deny and every matching `forbid` rule carries `@effect("ignore")` THE SYSTEM SHALL give the request the outcome Ignored, leave the item out of the box, and continue the activation.
  tier:     T2
  verify:   just test-one upol_outcome_ignored
  property: for every Deny whose matched rules are all marked ignore, the outcome is Ignored.
  harness:  kani_outcome_mapping_is_total, exhaustive to an unwind bound of 5, which covers every Cedar answer with up to 4 matched rules, each with or without the ignore mark, with and without an evaluation error. Requires the mapping to be a pure function over a summary of Cedar's response, separate from the call into Cedar.

- **UPOL-011** WHEN Cedar answers Deny and a matching `forbid` rule carries no `@effect("ignore")` THE SYSTEM SHALL give the request the outcome Denied and fail the activation with exit 5.
  tier:     T2
  verify:   just test-one upol_outcome_denied
  property: for every Deny with at least one matched rule not marked ignore, the outcome is Denied.
  harness:  kani_outcome_mapping_is_total, exhaustive to an unwind bound of 5, which covers every Cedar answer with up to 4 matched rules, each with or without the ignore mark, with and without an evaluation error. Requires the mapping to be a pure function over a summary of Cedar's response, separate from the call into Cedar.

- **UPOL-012** WHEN Cedar answers Deny and no rule matches the request THE SYSTEM SHALL give the request the outcome Undecided and ask the user.
  tier:     T2
  verify:   just test-one upol_outcome_undecided
  property: for every Deny with no matched rule and no evaluation error, the outcome is Undecided.
  harness:  kani_outcome_mapping_is_total, exhaustive to an unwind bound of 5, which covers every Cedar answer with up to 4 matched rules, each with or without the ignore mark, with and without an evaluation error. Requires the mapping to be a pure function over a summary of Cedar's response, separate from the call into Cedar.

- **UPOL-013** IF any policy fails to evaluate for a request THEN THE SYSTEM SHALL give the request the outcome Denied, whatever the other policies answer.
  tier:     T2
  verify:   just test-one upol_evaluation_error_denies
  property: for every answer that carries an evaluation error, the outcome is Denied, whatever else matched.
  harness:  kani_outcome_mapping_is_total, exhaustive to an unwind bound of 5, which covers every Cedar answer with up to 4 matched rules, each with or without the ignore mark, with and without an evaluation error. Requires the mapping to be a pure function over a summary of Cedar's response, separate from the call into Cedar.

- **UPOL-014** IF a policy fails to evaluate for a request THEN THE SYSTEM SHALL name that policy's `@id` in the error, when the policy has one.
  tier:     T0
  verify:   just test-one upol_evaluation_error_names_id

- **UPOL-015** THE SYSTEM SHALL identify a loadout as `Loadout::"<canonical path of its root directory>"`.
  tier:     T0
  verify:   just test-one upol_loadout_entity_id

- **UPOL-016** WHERE a loadout uses the flat layout THE SYSTEM SHALL identify it by the canonical path of `<name>/`, as if its file were `<name>/loadout.toml`.
  tier:     T0
  verify:   just test-one upol_flat_loadout_entity_id

- **UPOL-017** THE SYSTEM SHALL identify a project as `Project::"<canonical path of its root>"`, so that every path that reaches the project gives one id.
  tier:     T0
  verify:   just test-one upol_project_entity_id_canonical

- **UPOL-018** THE SYSTEM SHALL attribute to the project every item that the box's expanded entry carries, including what `[defaults]` and `[defaults.<type>]` contribute.
  tier:     T0
  verify:   just test-one upol_project_owns_expanded_entry_items

- **UPOL-019** THE SYSTEM SHALL give a loadout the attributes `path`, `sha256`, `packages`, `name` and an optional `description`.
  tier:     T0
  verify:   just test-one upol_loadout_attributes

- **UPOL-020** THE SYSTEM SHALL give a project the attributes `path`, `sha256`, `upstream_url` and an optional `upstream_commit`.
  tier:     T0
  verify:   just test-one upol_project_attributes

- **UPOL-021** THE SYSTEM SHALL give a loadout one tag for each variable it declares, whose value is the declared literal value, or `""` for an inherited variable.
  tier:     T0
  verify:   just test-one upol_loadout_variable_tags

- **UPOL-022** THE SYSTEM SHALL give every request a `context.box` that holds the box's entry name, its Box Type, the packages and variable names of its expanded entry, and the digest of its expanded Box Spec when the expansion computes one.
  tier:     T0
  verify:   just test-one upol_request_context_box

- **UPOL-023** THE SYSTEM SHALL apply a rule that tests `context.box.name` only to requests whose box has that entry name.
  tier:     T0
  verify:   just test-one upol_box_limited_rule_decides_only_its_box

- **UPOL-024** WHEN a box is created from the implicit `[sessions.default]` entry THE SYSTEM SHALL set `context.box.name` to `"default"`.
  tier:     T0
  verify:   just test-one upol_implicit_entry_box_name

- **UPOL-025** THE SYSTEM SHALL compute a principal's content hash as SHA-256 over the bytes `minimal-source-hash-v1\0` followed by the 32-byte git tree object id of its files in git's SHA-256 object format.
  tier:     T0
  verify:   just test-one upol_content_hash_definition

- **UPOL-026** THE SYSTEM SHALL compute the content hash from the files on disk, with no git repository present.
  tier:     T0
  verify:   just test-one upol_content_hash_without_git_repo

- **UPOL-027** THE SYSTEM SHALL include hidden files in the content hash, and leave out `.git`, the paths that the principal's own `.gitignore` files match, and the paths that the upload excludes.
  tier:     T0
  verify:   just test-one upol_content_hash_file_set

- **UPOL-028** THE SYSTEM SHALL compute the content hash with no input from a global gitignore, `.git/info/exclude` or `.ignore` file.
  tier:     T0
  verify:   just test-one upol_content_hash_ignores_machine_ignore_files

- **UPOL-029** THE SYSTEM SHALL hash a symlink as its target text, not as the file it points to.
  tier:     T0
  verify:   just test-one upol_content_hash_symlink_as_target_text

- **UPOL-030** WHEN a principal's files are a clean checkout of a SHA-256 git repository with no untracked files, no upload exclusions, no tracked files that match `.gitignore`, no submodules and no clean or smudge filters THE SYSTEM SHALL compute a tree id equal to `git rev-parse HEAD^{tree}`.
  tier:     T0
  verify:   just test-one upol_content_hash_matches_git_tree

- **UPOL-031** THE SYSTEM SHALL give a flat-layout loadout the content hash of a directory-layout loadout with the same files, with `<name>.toml` hashed as `<name>/loadout.toml`.
  tier:     T0
  verify:   just test-one upol_content_hash_flat_equals_directory

- **UPOL-032** WHEN a file of a principal is renamed, moved, or changes its executable bit or its contents THE SYSTEM SHALL compute a different content hash.
  tier:     T0
  verify:   just test-one upol_content_hash_changes_with_files

- **UPOL-033** THE SYSTEM SHALL compute the content hash of a principal before the first gate that needs it, on the activation path and on the task path.
  tier:     T0
  verify:   just test-one upol_content_hash_before_first_gate

- **UPOL-034** IF a file of a principal cannot be read while the content hash is computed THEN THE SYSTEM SHALL fail the activation or the task.
  tier:     T0
  verify:   just test-one upol_unreadable_file_fails

- **UPOL-035** WHEN `min policy hash <path>` runs on the root directory of a loadout or a project THE SYSTEM SHALL print that principal's content hash.
  tier:     T0
  verify:   just test-one upol_policy_hash_prints_hash

- **UPOL-036** WHERE the loadout uses the flat layout, WHEN `min policy hash` runs on its directory `<name>/` THE SYSTEM SHALL print the hash of that loadout.
  tier:     T0
  verify:   just test-one upol_policy_hash_flat_loadout

- **UPOL-037** IF `min policy hash` is given the path of a `.toml` file THEN THE SYSTEM SHALL refuse it with a message that names the directory to pass in its place.
  tier:     T0
  verify:   just test-one upol_policy_hash_refuses_toml_path

- **UPOL-150** IF the user policy file does not parse or does not validate THEN THE SYSTEM SHALL still run `min policy convert` and `min policy hash`.
  tier:     T0
  verify:   just test-one upol_policy_commands_run_with_broken_policy

- **UPOL-038** WHEN a patch would copy a host file into a box THE SYSTEM SHALL decide a `readHostFile` request on `HostFile::"<canonical path>"`, with the loadout that declares the patch, or the project, as principal.
  tier:     T0
  verify:   just test-one upol_patch_file_read_request

- **UPOL-039** WHEN a host file that a patch would copy is a symlink THE SYSTEM SHALL decide two `readHostFile` requests: one on the absolute, unresolved path of the link and one on the canonical path of its target.
  tier:     T0
  verify:   just test-one upol_symlink_two_requests

- **UPOL-040** THE SYSTEM SHALL copy a symlinked host file into the box only when both of its requests are Allowed.
  tier:     T0
  verify:   just test-one upol_symlink_needs_both_allowed

- **UPOL-041** IF either request for a symlinked host file is Denied THEN THE SYSTEM SHALL fail the activation with exit 5.
  tier:     T0
  verify:   just test-one upol_symlink_denied_fails

- **UPOL-042** IF either request for a symlinked host file is Ignored and neither is Denied THEN THE SYSTEM SHALL leave the file out of the box and continue the activation.
  tier:     T0
  verify:   just test-one upol_symlink_ignored_left_out

- **UPOL-043** THE SYSTEM SHALL match a rule against the absolute path that the request names, with no expansion of `~/` or `$NAME` in the rule.
  tier:     T0
  verify:   just test-one upol_rules_not_expanded

- **UPOL-044** WHEN a loadout or a project inherits a variable whose value comes from the host environment THE SYSTEM SHALL decide a `readHostVariable` request on `HostVariable::"<name>"`, with that loadout or the project as principal.
  tier:     T0
  verify:   just test-one upol_inherited_variable_read_request

- **UPOL-045** WHEN an inherited variable falls back to its default value THE SYSTEM SHALL decide a `setBoxVariable` request for it in place of a `readHostVariable` request.
  tier:     T0
  verify:   just test-one upol_default_fallback_is_set_request

- **UPOL-046** WHEN a `readHostVariable` request is Undecided THE SYSTEM SHALL ask the user with a prompt that names the principal and the variable.
  tier:     T0
  verify:   just test-one upol_read_variable_prompt_names

- **UPOL-047** WHEN a loadout, a package or a project sets a literal variable in a box THE SYSTEM SHALL decide a `setBoxVariable` request on `BoxVariable::"<name>"`, with that principal.
  tier:     T0
  verify:   just test-one upol_literal_variable_set_request

- **UPOL-048** WHEN a `setBoxVariable` request is Undecided THE SYSTEM SHALL ask the user with a prompt that shows the value the variable would be set to.
  tier:     T0
  verify:   just test-one upol_set_variable_prompt_shows_value

- **UPOL-049** IF a `setBoxVariable` request is Denied THEN THE SYSTEM SHALL fail the activation with exit 5 and a message that names the principal, the variable and the `@id` of the deciding rule.
  tier:     T0
  verify:   just test-one upol_set_variable_denied_names

- **UPOL-050** THE SYSTEM SHALL decide each loadout's variables and patches before it merges the loadouts.
  tier:     T0
  verify:   just test-one upol_gate_before_loadout_merge

- **UPOL-051** WHEN two loadouts set one variable to different values and a rule makes that variable Ignored THE SYSTEM SHALL compose the box with the variable left out and report no conflict.
  tier:     T0
  verify:   just test-one upol_ignored_variable_resolves_conflict

- **UPOL-052** THE SYSTEM SHALL keep the host values of loadout variables and the host files of loadout patches on the client until the daemon names every package in the box from a non-default upstream.
  tier:     T0
  verify:   just test-one upol_loadout_host_values_held

- **UPOL-053** WHEN a `readHostFile` or `readHostVariable` request is Allowed THE SYSTEM SHALL decide a `coexistWithHostFile` or `coexistWithHostVariable` request on that item for each principal in the box: the project, each loadout and each package from a non-default upstream.
  tier:     T0
  verify:   just test-one upol_coexist_requests_for_every_principal

- **UPOL-054** THE SYSTEM SHALL put a host file or a host variable into the box only when every coexist request on it is Allowed.
  tier:     T1
  verify:   just test-one upol_coexist_all_allowed
  property: for every list of coexist outcomes, of any length, the item enters only when every outcome is Allowed, and the activation fails when any outcome is Denied.

- **UPOL-055** IF a coexist request is Denied THEN THE SYSTEM SHALL fail the activation with exit 5 and a message that names the principal and the item.
  tier:     T1
  verify:   just test-one upol_coexist_denied_fails
  property: for every list of coexist outcomes, of any length, the item enters only when every outcome is Allowed, and the activation fails when any outcome is Denied.

- **UPOL-056** WHEN a coexist request is Undecided THE SYSTEM SHALL ask the user with a prompt that names the principal.
  tier:     T0
  verify:   just test-one upol_coexist_prompt_names_principal

- **UPOL-057** THE SYSTEM SHALL identify a package from an upstream other than `https://github.com/gominimal/pkgs` as `Package::"<upstream url>#<package name>"`, with the attributes `branch` and `locked_commit`.
  tier:     T0
  verify:   just test-one upol_package_entity

- **UPOL-058** WHEN the daemon resolves a box's contributions THE SYSTEM SHALL send `min` every package in the box from a non-default upstream, with its upstream URL, branch and locked commit.
  tier:     T0
  verify:   just test-one upol_daemon_names_non_default_packages

- **UPOL-059** THE SYSTEM SHALL make no request with a package from `https://github.com/gominimal/pkgs` as principal or as `installPackage` resource.
  tier:     T0
  verify:   just test-one upol_default_upstream_makes_no_request

- **UPOL-060** WHEN a project's box or a loadout lists a package from a non-default upstream THE SYSTEM SHALL decide an `installPackage` request on that package, with the project or the loadout as principal.
  tier:     T0
  verify:   just test-one upol_install_package_request

- **UPOL-061** WHEN a package from a non-default upstream depends on a package from a non-default upstream THE SYSTEM SHALL decide an `installPackage` request on the dependency, with the first package as principal.
  tier:     T0
  verify:   just test-one upol_install_dependency_request

- **UPOL-062** IF an `installPackage` request is Denied THEN THE SYSTEM SHALL fail the activation with exit 5 and a message that names the principal and the package.
  tier:     T0
  verify:   just test-one upol_install_denied_fails

- **UPOL-063** WHEN an `installPackage` request is Ignored THE SYSTEM SHALL leave the package out of the box and continue the activation.
  tier:     T0
  verify:   just test-one upol_install_ignored_left_out

- **UPOL-064** IF a lifecycle hook declaration in `minimal.toml` or in a loadout has no `name` THEN THE SYSTEM SHALL fail the parse with exit 3 and a message that names the file and the position of the hook.
  tier:     T0
  verify:   just test-one upol_hook_without_name_fails

- **UPOL-065** IF two lifecycle hooks in one project or one loadout have the same `name` THEN THE SYSTEM SHALL fail the parse with exit 3 and a message that names both hooks.
  tier:     T0
  verify:   just test-one upol_duplicate_hook_name_fails

- **UPOL-066** THE SYSTEM SHALL show the required `name` key in the lifecycle-hooks spec, the loadouts reference and the `minimal.toml` reference.
  tier:     T0
  verify:   just test-one upol_docs_show_hook_name

- **UPOL-067** THE SYSTEM SHALL state in the release notes of the release that adds the user policy that hook declarations require `name`.
  tier:     none
  verify:   none, the release notes are written when the release is cut, and no file in the tree holds them before then (gominimal/inbox#1231 S10)

- **UPOL-068** WHEN a box is activated THE SYSTEM SHALL decide one `executeLifecycleHook` request on `LifecycleHook::"<source>#<hook>:<event>"` for each event of each hook, before any hook runs, with the declaring loadout or the project as principal.
  tier:     T0
  verify:   just test-one upol_hook_request_per_event

- **UPOL-069** THE SYSTEM SHALL run a hook for a later event, such as `on_attach` or `on_destroy`, under the outcome decided at activation.
  tier:     T0
  verify:   just test-one upol_later_hook_events_use_activation_outcome

- **UPOL-070** THE SYSTEM SHALL deliver the outcome for each hook and event to the daemon as a separate entry.
  tier:     T0
  verify:   just test-one upol_hook_verdict_per_event

- **UPOL-071** THE SYSTEM SHALL run a hook for an event only when its request is Allowed.
  tier:     T0
  verify:   just test-one upol_hook_runs_only_when_allowed

- **UPOL-072** IF an `executeLifecycleHook` request is Denied THEN THE SYSTEM SHALL fail the activation with exit 5.
  tier:     T0
  verify:   just test-one upol_hook_denied_fails

- **UPOL-073** WHEN an `executeLifecycleHook` request is Ignored THE SYSTEM SHALL skip the hook for that event and continue the activation.
  tier:     T0
  verify:   just test-one upol_hook_ignored_skipped

- **UPOL-074** WHEN a principal has one or more Undecided hook requests THE SYSTEM SHALL ask the user once for that principal, in a prompt that lists each undecided hook by name and event and takes one answer for all of them.
  tier:     T0
  verify:   just test-one upol_hook_prompt_once_per_principal

- **UPOL-075** WHEN the user answers a hook prompt with a permanent answer THE SYSTEM SHALL write one rule for each listed hook and event, each with an `@id`, the exact principal, action and resource, and a `sha256` pin.
  tier:     T0
  verify:   just test-one upol_hook_permanent_answer_rule_per_hook

- **UPOL-076** THE SYSTEM SHALL read loadouts only from `<config>/minimal/loadouts/`.
  tier:     T0
  verify:   just test-one upol_one_loadouts_directory

- **UPOL-077** THE SYSTEM SHALL decide every loadout's requests through the user policy, whatever directory the loadout is in.
  tier:     T0
  verify:   just test-one upol_no_loadout_skips_gate

- **UPOL-078** WHERE a rule permits every action for `principal == Loadout::"<path>"` THE SYSTEM SHALL allow every request from that loadout that no `forbid` rule matches.
  tier:     T0
  verify:   just test-one upol_loadout_trust_rule

- **UPOL-079** THE SYSTEM SHALL show the loadout trust rule, with and without a `sha256` pin, in the loadouts concept page and the user-policy reference.
  tier:     T0
  verify:   just test-one upol_docs_show_trust_rule

- **UPOL-080** THE SYSTEM SHALL compose no built-in loadout.
  tier:     T0
  verify:   just test-one upol_no_builtin_loadout

- **UPOL-081** WHEN a box is created with no loadouts THE SYSTEM SHALL show the orientation banner and the shaped prompt in its shell, with no request to the user policy.
  tier:     T0
  verify:   just test-one upol_banner_without_loadout

- **UPOL-082** THE SYSTEM SHALL leave `default (built-in)` out of `MINIMAL_LOADOUTS` and out of the banner.
  tier:     T0
  verify:   just test-one upol_no_builtin_in_loadouts_env

- **UPOL-083** WHEN a loadout sets `PS1` or `PROMPT_COMMAND` and its `setBoxVariable` request is Allowed THE SYSTEM SHALL use the loadout's value in the box shell.
  tier:     T0
  verify:   just test-one upol_loadout_prompt_override

- **UPOL-084** WHEN `min` with the global `--no-input` flag creates a box with no loadouts, under an empty policy, for a project that declares no variables, patches or hooks THE SYSTEM SHALL create the box and exit 0.
  tier:     T0
  verify:   just test-one upol_empty_project_no_input_succeeds

- **UPOL-085** WHEN the user policy asks the user about a request THE SYSTEM SHALL offer allow once, ignore once, abort, allow permanently, ignore permanently and deny permanently.
  tier:     T0
  verify:   just test-one upol_prompt_choices

- **UPOL-086** WHEN the user gives a permanent answer THE SYSTEM SHALL append one rule with `@id("prompt-<date>-<n>")` that names the exact principal, action and resource.
  tier:     T0
  verify:   just test-one upol_permanent_answer_appends_rule

- **UPOL-087** WHERE the principal of a permanent answer is a loadout or a project THE SYSTEM SHALL add `principal.sha256 == "<hash>"` to the `when` of the rule it writes.
  tier:     T0
  verify:   just test-one upol_permanent_answer_pins_hash

- **UPOL-088** WHEN the user answers ignore permanently THE SYSTEM SHALL write a `forbid` rule that carries `@effect("ignore")`.
  tier:     T0
  verify:   just test-one upol_ignore_permanently_rule

- **UPOL-089** WHEN the user gives a permanent answer on a project's request THE SYSTEM SHALL ask for the scope, with "only this box" offered first and "every box in this project" second.
  tier:     T0
  verify:   just test-one upol_permanent_answer_scope_question

- **UPOL-090** WHEN the user picks "only this box" THE SYSTEM SHALL add `context.box.name == "<entry name>"` to the `when` of the rule it writes.
  tier:     T0
  verify:   just test-one upol_only_this_box_rule

- **UPOL-091** WHEN `min` appends a rule to `user_policy.cedar` THE SYSTEM SHALL leave every other byte of the file unchanged.
  tier:     T1
  verify:   just test-one upol_append_preserves_file
  property: for every policy file text and every appended rule, the old text is an exact prefix of the new text.

- **UPOL-092** THE SYSTEM SHALL offer at the request prompt no rule wider than the request.
  tier:     T0
  verify:   just test-one upol_prompt_offers_no_wider_rule

- **UPOL-093** WHEN a rule whose `@id` starts with `prompt-` or `cli-` matches a request in all but its `sha256` THE SYSTEM SHALL say in the prompt that the principal changed since it was approved, and name that rule's `@id`.
  tier:     T0
  verify:   just test-one upol_prompt_says_principal_changed

- **UPOL-094** WHEN no such rule nearly matches an Undecided request THE SYSTEM SHALL present the request as a first request.
  tier:     T0
  verify:   just test-one upol_prompt_first_request

- **UPOL-095** WHEN a loadout or a project makes an Undecided request and no rule has `principal == <that principal>` in its scope THE SYSTEM SHALL ask about the principal with the trust prompt before it asks about the request.
  tier:     T0
  verify:   just test-one upol_trust_prompt_for_new_principal

- **UPOL-096** WHEN a loadout or a project makes an Undecided request and the only rules with `principal == <that principal>` are pinned rules with a `prompt-` or `cli-` `@id` whose `sha256` no longer matches THE SYSTEM SHALL ask with the trust prompt and say that the principal changed since it was approved.
  tier:     T0
  verify:   just test-one upol_trust_prompt_for_changed_principal

- **UPOL-097** WHEN the trust prompt asks about a project THE SYSTEM SHALL name the box the request is for and offer the scope "every box in this project" or "only this box" before the trust choices.
  tier:     T0
  verify:   just test-one upol_trust_prompt_project_scope

- **UPOL-098** WHEN the trust prompt asks about a principal THE SYSTEM SHALL offer three choices: trust this scope, trust this scope at its current contents, and no blanket approval.
  tier:     T0
  verify:   just test-one upol_trust_prompt_choices

- **UPOL-099** WHEN the user picks trust this scope THE SYSTEM SHALL write `permit(principal == <principal>, action, resource);` with an `@id`, and with `context.box.name == "<entry name>"` in its `when` when the scope is one box.
  tier:     T0
  verify:   just test-one upol_trust_rule_written

- **UPOL-100** WHEN the user picks trust this scope at its current contents THE SYSTEM SHALL write the same rule with `principal.sha256 == "<hash>"` in its `when`.
  tier:     T0
  verify:   just test-one upol_pinned_trust_rule_written

- **UPOL-101** WHEN the user picks no blanket approval THE SYSTEM SHALL continue with the request prompt for each Undecided request.
  tier:     T0
  verify:   just test-one upol_no_blanket_approval_continues

- **UPOL-102** WHEN the user picks a trust choice and a stale pinned trust rule for the principal exists THE SYSTEM SHALL replace that rule in place, keep its `@id`, and leave every other byte of the file unchanged.
  tier:     T1
  verify:   just test-one upol_stale_trust_rule_replaced
  property: for every policy file text that holds a stale pinned trust rule, every byte outside that rule is unchanged after the replace, and the rule keeps its @id.

- **UPOL-103** THE SYSTEM SHALL ask about a package from a non-default upstream only with the request prompt, never with the trust prompt.
  tier:     T0
  verify:   just test-one upol_no_trust_prompt_for_packages

- **UPOL-104** WHEN the trust prompt offers its first two choices THE SYSTEM SHALL say that the rule permits every action for the scope, name the boxes of the project that it covers, and say that `forbid` rules still win.
  tier:     T0
  verify:   just test-one upol_trust_prompt_states_effect

- **UPOL-105** WHEN the user picks a trust choice THE SYSTEM SHALL decide that principal's requests again under the new rule, and ask only about those still Undecided.
  tier:     T0
  verify:   just test-one upol_trust_choice_redecides

- **UPOL-106** WHILE `--no-prompt` or the global `--no-input` flag is set, or no terminal is attached, THE SYSTEM SHALL show no trust prompt.
  tier:     T0
  verify:   just test-one upol_headless_no_trust_prompt

- **UPOL-107** WHILE `--no-prompt` or the global `--no-input` flag is set, or no terminal is attached, IF a request is Undecided THEN THE SYSTEM SHALL fail the activation with exit 2.
  tier:     T0
  verify:   just test-one upol_headless_undecided_fails

- **UPOL-108** WHILE `--no-prompt` or the global `--no-input` flag is set, or no terminal is attached, IF a request is Undecided THEN THE SYSTEM SHALL print one ready-to-paste Cedar rule for each Undecided request and name `<config>/minimal/user_policy.cedar`.
  tier:     T0
  verify:   just test-one upol_headless_prints_rules

- **UPOL-109** WHEN `min` prints a rule for a project's request THE SYSTEM SHALL limit the rule to the box with `context.box.name`.
  tier:     T0
  verify:   just test-one upol_printed_rule_box_limited

- **UPOL-110** WHEN `min` prints rules in one run THE SYSTEM SHALL give each one `@id("cli-<UTC date and time to the second>-<4 random hex digits>-<n>")`, with one random part for the run and `<n>` counting the rules it prints.
  tier:     T0
  verify:   just test-one upol_printed_rule_ids_unique

- **UPOL-111** IF `min dash` meets an Undecided request THEN THE SYSTEM SHALL refuse it with a one-line error and show no prompt.
  tier:     T0
  verify:   just test-one upol_dash_refuses_undecided

- **UPOL-112** IF a request is Denied THEN THE SYSTEM SHALL name in the error the `@id` of each matching `forbid` rule, or its position in the file when it has no `@id`.
  tier:     T0
  verify:   just test-one upol_denial_names_rule

- **UPOL-113** WHEN `min` returns its verdicts to the daemon THE SYSTEM SHALL carry the `@id` of the deciding rule with each verdict.
  tier:     T0
  verify:   just test-one upol_verdict_carries_rule_id

- **UPOL-114** WHEN a loadout item is Ignored THE SYSTEM SHALL send it to the daemon as an Ignored verdict, without its value.
  tier:     T0
  verify:   just test-one upol_ignored_loadout_item_sent_without_value

- **UPOL-115** THE SYSTEM SHALL keep each Ignored item in the box record, with its principal, its resource and the `@id` of the deciding rule, for as long as the box record exists.
  tier:     T0
  verify:   just test-one upol_box_record_keeps_ignored_items

- **UPOL-116** WHEN `min box show <box>` runs THE SYSTEM SHALL list the box's Ignored items in a user-policy section, and include them in `-o json`.
  tier:     T0
  verify:   just test-one upol_box_show_lists_ignored_items

- **UPOL-117** THE SYSTEM SHALL describe in spec 29 BOX-159 that the box record carries the Ignored items, and in spec 28 BCLI-003 that `min box show` renders them.
  tier:     T0
  verify:   just test-one upol_specs_carry_ignored_items

- **UPOL-118** IF `user_policy.toml` exists and `user_policy.cedar` does not THEN THE SYSTEM SHALL refuse the activation with exit 3 and a message that says the policy file must be converted and names `min policy convert`.
  tier:     T0
  verify:   just test-one upol_toml_without_cedar_refuses

- **UPOL-119** WHEN `min policy convert` runs THE SYSTEM SHALL write `user_policy.cedar` from the rules in `user_policy.toml` and leave `user_policy.toml` in place.
  tier:     T0
  verify:   just test-one upol_convert_writes_cedar_keeps_toml

- **UPOL-120** WHEN `min policy convert` converts a glob THE SYSTEM SHALL write a `like` condition on `resource.path` or `resource.name`, with `~/` expanded to the home directory of the user who runs the command.
  tier:     T1
  verify:   just test-one upol_convert_glob_to_like
  property: for every glob in the convertible set and every absolute path, the glob matches the path under globset defaults exactly when the converted like pattern matches it under Cedar.

- **UPOL-121** IF a TOML rule cannot be expressed exactly THEN THE SYSTEM SHALL write it into the output file as a comment with the original rule and the reason.
  tier:     T0
  verify:   just test-one upol_convert_unconvertible_as_comment

- **UPOL-122** WHEN `min policy convert` finishes THE SYSTEM SHALL print how many rules it converted and how many it did not.
  tier:     T0
  verify:   just test-one upol_convert_prints_counts

- **UPOL-123** WHEN `min policy convert` converts an allow, a deny or an ignore rule THE SYSTEM SHALL write a `permit`, a `forbid`, or a `forbid` with `@effect("ignore")`.
  tier:     T0
  verify:   just test-one upol_convert_rule_kinds

- **UPOL-124** WHEN `min policy convert` converts a `[hooks]` rule for an exact project root THE SYSTEM SHALL write it with `principal == Project::"<root>"` and `action == Action::"executeLifecycleHook"`.
  tier:     T0
  verify:   just test-one upol_convert_hooks_exact_root

- **UPOL-125** WHEN `min policy convert` converts a `[hooks]` rule with a root glob THE SYSTEM SHALL write it with `principal is Project` and `principal.path like "<expanded glob>"`.
  tier:     T1
  verify:   just test-one upol_convert_hooks_root_glob
  property: for every glob in the convertible set and every absolute path, the glob matches the path under globset defaults exactly when the converted like pattern matches it under Cedar.

- **UPOL-126** THE SYSTEM SHALL write a converted policy that validates against the schema.
  tier:     T0
  verify:   just test-one upol_converted_policy_validates

- **UPOL-127** WHEN an activation starts and neither `user_policy.cedar` nor `user_policy.toml` exists THE SYSTEM SHALL write `user_policy.cedar` from the template that ships with `min`.
  tier:     T0
  verify:   just test-one upol_default_policy_written

- **UPOL-128** THE SYSTEM SHALL ship a default policy that validates against the schema and contains no `permit` rule.
  tier:     T0
  verify:   just test-one upol_default_policy_validates_no_permit

- **UPOL-129** THE SYSTEM SHALL write into the default policy active `forbid` rules on every action for a `HostFile` under `~/.ssh`, `~/.aws` and `~/.gnupg`, with `~` expanded to the user's home directory.
  tier:     T0
  verify:   just test-one upol_default_policy_forbids_secret_dirs

- **UPOL-130** THE SYSTEM SHALL write into the default policy a `forbid` rule with `@effect("ignore")` for `.DS_Store` files.
  tier:     T0
  verify:   just test-one upol_default_policy_ignores_ds_store

- **UPOL-131** THE SYSTEM SHALL write into the default policy commented examples of a loadout trust rule, a pinned project rule and a project rule limited to one box.
  tier:     T0
  verify:   just test-one upol_default_policy_examples

- **UPOL-132** WHEN `min task run` runs a task whose `env_vars` inherit a value from the host THE SYSTEM SHALL decide a `readHostVariable` request for each one, with the project as principal and the task's entry name in `context.box.name`, before it reads the value.
  tier:     T0
  verify:   just test-one upol_task_host_variable_request

- **UPOL-133** WHEN `min task run` runs a task with a literal variable THE SYSTEM SHALL decide a `setBoxVariable` request for it.
  tier:     T0
  verify:   just test-one upol_task_literal_variable_request

- **UPOL-134** WHEN a task's `readHostVariable` request is Allowed THE SYSTEM SHALL put the value into the task only after every coexist request on it, for every principal in the task's box, is Allowed.
  tier:     T0
  verify:   just test-one upol_task_variable_coexist

- **UPOL-135** WHEN `min task run` decides a request THE SYSTEM SHALL evaluate rules pinned to the project's `sha256` against the project's current content hash.
  tier:     T0
  verify:   just test-one upol_task_pinned_rules

- **UPOL-136** IF in-box `min run` runs a task whose `env_vars` inherit a host value THEN THE SYSTEM SHALL refuse the task with exit 2 and a message that says the task reads variables from the host and names `min task run <task>`.
  tier:     T0
  verify:   just test-one upol_in_box_run_refuses_host_variables

- **UPOL-137** WHEN `min task run` decides a request THE SYSTEM SHALL apply the same outcomes, prompt, trust prompt and headless refusal as an activation, and write permanent answers to `user_policy.cedar`.
  tier:     T0
  verify:   just test-one upol_task_path_same_gate

- **UPOL-138** WHEN a task declares a read-only patch key with an absolute host path THE SYSTEM SHALL decide a `readHostFile` request on `HostFile::"<declared path>"`, with the project as principal and the task in `context.box`.
  tier:     T0
  verify:   just test-one upol_task_patch_read_request

- **UPOL-139** WHEN a task declares a read-write patch key with an absolute host path THE SYSTEM SHALL decide a `readHostFile` request and a `writeHostFile` request on it, and mount it only when both are Allowed.
  tier:     T0
  verify:   just test-one upol_task_patch_read_write_requests

- **UPOL-140** WHEN a task declares a patch key under `~/` THE SYSTEM SHALL make no request for it.
  tier:     T0
  verify:   just test-one upol_task_home_patch_no_request

- **UPOL-141** WHEN `min task run` starts a task that declares patches THE SYSTEM SHALL send the daemon a verdict that names each allowed key with its kind and mode, and each dropped key.
  tier:     T0
  verify:   just test-one upol_task_patch_verdict_sent

- **UPOL-142** THE SYSTEM SHALL mount for a task only the patch keys that the verdict allows.
  tier:     T2
  verify:   just test-one upol_task_mounts_only_allowed_keys
  property: for every set of declared patch keys and every verdict, the daemon mounts only keys the verdict allows with the same kind and mode, and refuses the task when the verdict is missing, omits a declared key, or differs in kind or mode.
  harness:  kani_task_patch_verdict_fails_closed, exhaustive to an unwind bound of 4, which covers up to 3 declared keys and 3 verdict entries with every kind and mode. Requires the check to be a pure function over the declared keys and the verdict, comparing keys by exact equality, separate from the mount.

- **UPOL-143** IF a task declares patches and the daemon received no verdict, or the task declares a key that the verdict does not name, or a key whose kind or mode differs from the verdict THEN THE SYSTEM SHALL refuse the task.
  tier:     T2
  verify:   just test-one upol_task_patch_verdict_fails_closed
  property: for every set of declared patch keys and every verdict, the daemon mounts only keys the verdict allows with the same kind and mode, and refuses the task when the verdict is missing, omits a declared key, or differs in kind or mode.
  harness:  kani_task_patch_verdict_fails_closed, exhaustive to an unwind bound of 4, which covers up to 3 declared keys and 3 verdict entries with every kind and mode. Requires the check to be a pure function over the declared keys and the verdict, comparing keys by exact equality, separate from the mount.

- **UPOL-144** IF in-box `min run` runs a task that declares patches THEN THE SYSTEM SHALL refuse the task with exit 2 and a message that says the task mounts host files and names `min task run <task>`.
  tier:     T0
  verify:   just test-one upol_in_box_run_refuses_patches

- **UPOL-145** WHEN a task lists a package from a non-default upstream THE SYSTEM SHALL decide an `installPackage` request on it, with the project as principal, before the daemon installs it.
  tier:     T0
  verify:   just test-one upol_task_install_package_request

- **UPOL-146** THE SYSTEM SHALL install for a task no package from a non-default upstream that the verdict does not allow.
  tier:     T0
  verify:   just test-one upol_task_installs_only_allowed_packages

- **UPOL-147** WHEN an activation meets an inherited variable THE SYSTEM SHALL decide its `readHostVariable` request before it reads the value from the environment.
  tier:     T0
  verify:   just test-one upol_activation_decides_before_reading

- **UPOL-148** THE SYSTEM SHALL describe in spec 29 the reading of loadout patches as `readHostFile` requests under the user policy (BOX-167, BOX-182), and the comparison of only allowed variables, with an `@effect("ignore")` rule as the remedy (BOX-183).
  tier:     T0
  verify:   just test-one upol_spec29_follows_policy

## Non-goals

- A command that explains a decision or lists rules by principal: the epic's Negotiable item. UPOL-112 already names the deciding rule. The command stays the implementer's choice, outside this spec.
- Gatehouse org policy: [`gatehouse-spec.md` §7](https://github.com/gominimal/arch/blob/327e1b57d16b31ae1f4249cb317e3bd5b22e98b8/specs/authn-authz/gatehouse-spec.md). It is a separate engine and schema, and this policy shares nothing with it.
- Retiring `[loadout-file-rules]` in the architecture of record, and the AT7 and AT8 wording that names it: gominimal/arch#178.
- A `name` on the hook examples in the architecture of record: gominimal/arch#179.
- `[package-file-rules]`, which gates the files that package programs read and write on the client at run time: no action in this policy covers it, so it stays in the architecture of record.
- `mip run` and its patch gate: gominimal/inbox#1128, with gominimal/minimal#3158.
- Package host mappings on the task path: gominimal/inbox#902.
- Whether tasks pick up `default_loadouts`: gominimal/inbox#781. Until that changes, UPOL-134 runs the coexist check for those loadouts.
- The command that replaces `min session run <box> <task>`, and its gate: gominimal/minimal#3216. Spec 28 BCLI-042 removes `min session run`.
- `@effect` values other than `"ignore"`: the epic limits the annotation to `"ignore"`, and UPOL-007 refuses the rest.
- The `dest` of a patch: the user-policy reference already leaves it to the user.

## Non-functional requirements

- **UPOL-149** WHEN the computation of the content hash changes THE SYSTEM SHALL use a new header version, and ship the change in a breaking release.
  tier:     none
  verify:   none, a future change to the hash computation cannot be observed today (gominimal/inbox#1231 S4)

## Design reasoning

The shape of the policy itself (Cedar, one file, the principals, the outcomes, the trust prompt and the clean break) is decided in the epic, gominimal/inbox#1231, and in the design it links. This section records the decisions taken while the spec was written.

**One spec, in this repository.** Every acceptance criterion names the CLI, the daemon or specs 28 and 29, and all of them are here. A sibling spec in the Gatehouse repository was the alternative. It was rejected because the epic keeps the user policy apart from the org policy. The two changes to the architecture of record are issues (gominimal/arch#178, #179), not a spec.

**Behaviour, not code paths.** The epic names functions, wire types and files in several criteria. The requirements state only what a test can observe, for example that the daemon names the non-default packages in its reply. Naming the code was the alternative. It was rejected because the spec then goes stale with every rename. The code paths go to the planning step.

**Documentation and spec edits are requirements.** The reference pages, the release notes and the amendments to specs 28 and 29 (UPOL-004, 066, 067, 079, 117, 148) are requirements, each checked by a test that reads the page. Leaving them to the plan was the alternative. It was rejected because nothing then checks that they land. These tests check that the text is present, not that it is right.

**Exit codes come from the architecture of record.** A denial exits 5, an undecided request with no prompt exits 2, and a policy that does not load exits 3, as the [exit-code table](https://github.com/gominimal/arch/blob/327e1b57d16b31ae1f4249cb317e3bd5b22e98b8/architecture.md#exit-codes) gives them. The epic gives no codes. Leaving them open was the alternative. It was rejected because a script then cannot tell a denial from a missing terminal. Two refusals had no row in the table. A `user_policy.toml` with no `user_policy.cedar` exits 3, because it is a configuration to fix. An in-box `min run` that must come from the host exits 2, as an operation its context does not admit. Exit 5 for all three was the alternative. It was rejected because it mixes "fix your setup" with "your rule said no".

**A policy that does not load stops requests, not repairs.** UPOL-006 fails every activation and every `min task run`. UPOL-150 keeps `min policy convert` and `min policy hash` working, because a user needs them to fix the file. Stopping every `min` command was the alternative, and it blocks that repair.

**One side of a symlink ignored, the other undecided.** The file is left out at once, with no prompt (UPOL-042). Asking first was the alternative. The answer cannot change the result, so the prompt would be a question the user cannot act on. This order is the one the outcome lattice already has: Denied before Ignored, Ignored before undecided.

**Tasks follow the epic's criteria, not the wider story.** S21a's story says "every way of running a task". Its criteria cover `min task run`, with a refusal for in-box `min run`. The requirements follow the criteria. A requirement that binds every future way of running a task was the alternative. It was rejected because the replacement for `min session run` (gominimal/minimal#3216) does not exist yet, and nothing could test the requirement.

**The rule for changing the hash is a non-functional requirement.** UPOL-149 says that a change to the hash computation uses a new header version and ships in a breaking release. It is at `tier: none`, because a future change cannot be observed today. Dropping it was the alternative, since UPOL-025's test fails on any change anyway. It was kept so that the intent stays written down.

**Verification tiers.** Most requirements stay at T0. Six groups were considered for more:

- The outcome mapping (UPOL-009 to 013) is at T2. It is the decision that the whole gate rests on, and a pure function over a summary of Cedar's answer can be exhausted. T1 and one test per row were the alternatives.
- The content hash (UPOL-030, 032) stays at T0, on hand-built trees. A property test that compares the result with `git write-tree` was the alternative. It was not taken, so a mismatch with git on an unusual tree can go unseen.
- Coexist (UPOL-054, 055) is at T1. A property test over principal outcome lists of any length checks that the item enters only when all are Allowed. The symlink pair stays at T0, because its 16 combinations fit in one test. The existing Kani harnesses on the outcome lattice in the policy crate prove its laws.
- The file edits (UPOL-091, 102) are at T1. A user's own formatting is where an edit goes wrong, and fixed files do not explore it.
- The glob conversion (UPOL-120, 125) is at T1. It compares `globset` with Cedar's `like` on generated globs and paths, which makes "converts exactly" a checked claim.
- The daemon's check on task mounts (UPOL-142, 143) is at T2. It is a fail-closed control over a small, bounded input. T1 and one test per refusal case were the alternatives.

No requirement is at T3. This repository has no Lean project, and Cedar's evaluator cannot be extracted. Kani already covers the one pure core worth a proof.

**Generality:** The gate holds for any host `min` creates a box on, because `min` decides every request before a host receives a value. The daemon side, which applies the verdicts and checks task mounts, is the same daemon on every host. Remote hosts cannot be tested today, and no requirement here depends on one.

## Security considerations

This policy is the client-side control for trust boundary TB1 in the [threat model](https://github.com/gominimal/arch/blob/327e1b57d16b31ae1f4249cb317e3bd5b22e98b8/architecture.md#threat-model) (client host and box: loadout patches and prompts). It mitigates AT7 (a malicious `minimal.toml`) and AT8 (a malicious or over-broad loadout). gominimal/arch#178 moves the architecture's wording for both from `[loadout-file-rules]` to this policy.

- **Invariant:** THE SYSTEM SHALL let a request that the user policy governs take effect only when its outcome is Allowed or the user approves it at the prompt.
  enforced by: `min`, which maps Cedar's answer to an outcome before any item enters a box, and the daemon, which applies only the verdicts `min` returns.
  covered by: UPOL-009, UPOL-010, UPOL-011, UPOL-012, UPOL-038, UPOL-044, UPOL-047, UPOL-060, UPOL-068, UPOL-071

- **Invariant:** THE SYSTEM SHALL give the outcome Denied to a request for which any policy fails to evaluate.
  enforced by: the outcome mapping in `min`, which does not let a skipped `forbid` turn into a pass.
  covered by: UPOL-013

- **Invariant:** THE SYSTEM SHALL keep a host value or a host file on the client until every principal in the box is allowed to coexist with it.
  enforced by: `min`, which holds loadout and task host values until it knows every principal.
  covered by: UPOL-052, UPOL-053, UPOL-054, UPOL-134

- **Invariant:** THE SYSTEM SHALL mount for a task no host path that the verdict from `min` does not allow, with the same kind and mode.
  enforced by: the daemon, which refuses the task when the verdict is missing or does not match.
  covered by: UPOL-142, UPOL-143

- **Invariant:** THE SYSTEM SHALL match every rule against the path or name that the request names, with no expansion that a principal can influence.
  enforced by: rules that hold absolute paths, with no `~/` or `$NAME` expansion at evaluation time.
  covered by: UPOL-043

- **Invariant:** THE SYSTEM SHALL write from a request prompt no rule wider than the request.
  enforced by: the prompt, which writes the exact principal, action and resource. The trust prompt is the one exception, and the epic accepts it as a security trade-off.
  covered by: UPOL-086, UPOL-092

## Open questions

- [NEEDS CLARIFICATION (MEDIUM): When a `forbid` rule marked `@effect("ignore")` matches a coexist check, does the item stay out of the box while the activation continues, or does the mark have no meaning on a coexist check? The epic names the Allowed, Denied and Undecided cases only, and the question was left open while this spec was written.]

