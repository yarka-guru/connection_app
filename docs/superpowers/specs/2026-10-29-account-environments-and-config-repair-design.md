# Account-based environments, config repair, full project form, automatic DB auth

**Date:** 2026-10-29
**Status:** Draft, awaiting owner approval
**Baseline:** `feat/iam-auth-proxy` (PR #64, local PostgreSQL auth proxy), on top of v3.7.10
**Target release:** 3.8.0. PR #64 ships first as 3.7.11 and this work does not block it.

## Why

`projects.json` is shared between people (Settings → Export/Import), but an
environment is identified by the **name of the user's AWS profile**:

- `profileFilter` is a prefix match on the profile name and decides which
  profiles belong to a project;
- `envPortMapping` is a suffix match on the profile name and decides the
  local port.

This breaks in practice. With the owner's own config:

- `d-9067836ed2-231030592327-AdministratorAccess` points at the Covered prod
  account but is not recognised, because its name does not start with
  `covered`. `aws configure sso` suggests names like this by default, so a
  teammate who accepted the default sees nothing.
- `covered-replica` (a different account, not a database environment) shows
  up as an environment in all five Covered projects, on port 5432.
- A teammate whose dev profile is called `dev-covered` would get the prod
  port 5460, because the name ends in `covered`.
- Adding the dev environment meant editing ports in five projects. Before
  3.7.9 (#50) an unmapped profile was not shown at all, so the dev
  environment added in the app stayed invisible until `projects.json` was
  edited by hand.

Separately:

- One malformed project, or a JSON syntax error, makes `load_project_configs`
  fail and **every** project disappears. Validation only runs on save, so a
  hand-edited mistake surfaces later as a connect error.
- The Settings project form does not show every field: `secretPath`,
  `secretUsernameField`, `secretPasswordField` and `multiplexed` can only be
  changed in the file.
- PR #64 makes PostgreSQL sign-in pick an IAM token or the password based
  on what RDS asks for, but the form still offers only "Secrets Manager / IAM"
  and does not say this.

## Goals

1. An environment is identified by its **AWS account ID**. The same
   `projects.json` works for anyone, whatever their profiles are called.
2. Adding a database or an environment is done entirely in the app, in one
   place, including copying environments from another project.
3. The config is checked on every load. A broken project is visible with
   its errors and does not hide the others. Existing configs are converted
   to the new format by the app, with a backup.
4. Every field in a project config has a place in the UI.
5. A PostgreSQL connection works whether the database user needs IAM or a
   password, with no switching. The form makes this the default and says so.

## Non-goals

- Discovering environments or databases by calling AWS (scanning accounts,
  listing RDS in every profile).
- Changing the tunnel, the SSM protocol or the PostgreSQL proxy internals
  from #64, except adding the password-only upstream mode described under
  Authentication.
- An auth proxy for MySQL.
- Syncing configs through any server or cloud store; Export/Import stays a file.
- Windows.

## Anti-goals (a failure even if everything else works)

- **Connecting to the wrong account.** Before a tunnel opens, the account the
  credentials actually belong to must equal the environment's `accountId`.
- **Losing user data.** Migration and repair never delete or overwrite
  anything without a backup. A file that cannot be parsed is never rewritten
  automatically.
- **Silently moving an existing environment to another port.** DB clients
  have bookmarks on these ports. Migration keeps the port each profile gets
  today.
- **Slow or online-only startup.** Listing projects and environments reads
  local files only, with no AWS calls.
- **A behaviour change in how an existing PostgreSQL project signs in**
  compared to #64.

## Decided with the owner

- Environments are keyed by AWS account ID, not profile name.
- Environment names such as prod/staging/dev, a port per environment, and an
  optional per-environment `rdsPattern` override for when two environments
  share one account.
- The app finds the profile for an account itself. With several candidates
  the user picks one once and the choice is remembered. With none, the
  environment is shown greyed out with a "Create profile" action.
- The connection form shows environment names, not profile names.
- On first start of the new version the app converts the existing config to
  this format and keeps a backup. A broken project is shown on its own and
  does not hide the others.
- `covered-replica` is not migrated as an environment (see the migration
  rule for how this falls out generally).
- Auth: "Automatic" is the default for PostgreSQL projects; "IAM only" and
  "Password only" remain as explicit choices. The DB user comes from the
  secret, or from "IAM DB User" when there is no secret. Secret lookup
  (prefix, exact path, field names) becomes visible in the form. MySQL keeps
  the explicit IAM / password choice.

## Design

### Config format

Each project gains an ordered `environments` array:

```json
"covered": {
  "name": "Covered DB",
  "region": "us-west-1",
  "rdsPattern": "covered-db",
  "...": "other existing fields unchanged",
  "environments": [
    { "name": "prod",    "accountId": "231030592327", "port": "5460" },
    { "name": "staging", "accountId": "646770357179", "port": "5461" },
    { "name": "dev",     "accountId": "073813807522", "port": "5462",
      "rdsPattern": null, "region": null }
  ]
}
```

- `name`: unique within the project, non-empty, shell-safe.
- `accountId`: exactly 12 digits, stored as a string (leading zeros, as in
  `073813807522`).
- `port`: as today, a string of digits.
- Optional overrides of the project values for this environment only:
  `rdsPattern` and `region`, and for service projects `targetPattern`,
  `ecsCluster`, `ecsService` and `bastionPattern`. All of them are part of
  the format from day one, so two environments in one account need no
  later format change. *(Builder-check answer; my call.)*
- When `environments` is present and non-empty, `profileFilter` and
  `envPortMapping` are **ignored**. They are left in the file untouched, so
  an older app version reading the same file keeps working, but the form no
  longer shows them and new projects do not get them. `defaultPort` remains
  the suggested port for a new environment.
- A project without `environments` is a legacy project. It goes through
  migration (below). Until the user applies the migration it keeps working
  with today's rules, so nothing breaks on upgrade.

`ProjectConfig` has no `deny_unknown_fields`, so older versions ignore the
new key.

**A file written by 3.8 must still load in 3.7.x.** This matters for
teammates who have not updated, the Homebrew CLI and Import in an older app.
In 3.7.x `envPortMapping` and `defaultPort` are required, and Import rejects
any `authType` other than `secrets`/`iam`. So 3.8 always writes:
- `defaultPort`;
- `envPortMapping` (`{}` for projects created in 3.8);
- for projects created in 3.8, a `profileFilter` sentinel that matches no
  profile (`"__connection-app-3.8__"`), so an older app hides the project
  instead of giving it every unclaimed profile;
- `authType` only as `secrets` or `iam`. The password-only choice is a
  separate boolean, `passwordOnly` (see Authentication).

Migrated projects keep their original legacy fields, so they keep working
fully in 3.7.x. *(Builder-check answer; my call: the cost is small and the
alternative breaks every project for a teammate on 3.7.x.)*

### Account of a profile, and profile of an environment

The account of a profile, from `~/.aws/config` (respecting
`AWS_CONFIG_FILE`, as `get_aws_dir()` does today):

1. `sso_account_id`, including profiles that use an `sso_session` section;
2. otherwise the account in `role_arn` (`arn:<partition>:iam::<account>:role/...`);
3. otherwise unknown (static keys, `credential_process`, env credentials).
   The UI offers a "Detect account" action that calls
   `sts:GetCallerIdentity` with that profile and caches the result in
   `~/.connection-app/profile-accounts.json`, keyed by profile name, so the
   CLI sees it too. The cache entry is dropped when the profile's section in
   `~/.aws/config` changes.

Resolving an environment (local files only, no AWS calls):

- **0 candidates.** The environment is unavailable: greyed out in the
  connection form, with "No AWS profile for account 0738…7522" and a
  **Create profile** action. That action opens the existing AWS profile
  editor pre-filled with `sso_account_id`, plus `sso_start_url`,
  `sso_region` and `sso_role_name` copied from an existing SSO profile; the
  user can change any of it.
- **1 candidate.** It is used.
- **More than one** (the owner has two for 231030592327). The remembered
  choice for that **account** is used; without one, the first in
  `~/.aws/config` order. The connection form shows a small "via profile ▾"
  picker only in this case. The choice is stored per account in
  `~/.connection-app/preferences.json`, which is local and never exported.

**Account check at connect.** After SSO/credentials are ready and before the
tunnel opens, the connect path calls `sts:GetCallerIdentity` (reuse the call
if the connect path already makes one) and refuses with a clear error if
the account does not equal the environment's `accountId`.

### Which projects are listed

The connection form lists a project when at least one of its environments
resolves to a profile. Projects with no reachable environment, for example
from a team config imported by someone without access to those accounts, are
listed only in Settings, marked "No AWS access", so they do not clutter the
main screen.

### Connect, saved connections, CLI

- `connect` takes `environment` (the name) instead of `profile`, plus an
  optional `profile` override that must belong to the environment's account.
  The backend resolves the profile. Legacy projects keep the old
  `profile`-based path until migrated.
- Saved connections store `environment` and keep `profile` only as the
  remembered candidate. Existing saved connections are converted the first
  time they are read: profile → account → the environment in that project
  with that account. If that fails, the saved connection is marked "needs
  attention" with an Edit action and is not deleted.
- CLI: a new `--env <name>`. `--profile` still works and must belong to one
  of the project's environments. Interactive selection lists environment
  names with the resolved profile next to each. `connection-app-cli
  projects` prints environments, accounts, ports and resolved profiles. The
  CLI applies migration **in memory only** and never writes
  `projects.json`; it prints a one-line hint to open the app to save it.

### Loading, validation and repair

- **Per-project loading.** Parse the file as a JSON object, then deserialize
  each project separately. A project that fails to deserialize becomes a
  "broken" entry holding the error and its raw JSON. Valid projects load
  normally. Every loaded project is also run through
  `validate_project_config`, extended with the new rules (account ID format,
  unique environment names, port format, ports unique within a project).
- **What the user sees.** In the connection form a broken or invalid project
  is shown greyed out with a one-line reason, never hidden. In Settings it
  has the full error list and three actions: **Edit** (the form, pre-filled
  with whatever could be read), **Edit JSON** (a raw editor for that one
  project, validated before save) and **Delete** (with confirmation).
- **Whole-file syntax error.** Settings shows a banner with the parser's
  line and column, a raw editor for the whole `projects.json`, and
  **Restore backup** when a backup exists. The broken file is never
  rewritten automatically.
- **Saving.** Saving one project never fails because another project is
  broken: broken entries are written back byte-for-byte from their raw JSON.
  All writes are atomic (temp file in the same directory, then rename).
- **Safe automatic fixes on load** (applied in memory and written on the next
  save or migration apply): numbers where a string is expected (ports),
  surrounding whitespace, missing fields that have defaults. Anything that
  would change meaning is reported, not fixed.

### Import

Import goes through the same per-project loader:
- legacy entries are accepted and offered for migration;
- broken entries are reported, and the rest still import.

Before Import writes anything, `projects.json` is backed up like the
migration does. When an imported project has the same key as a local one,
the user picks per project: **Replace**, **Keep mine** or **Import as copy**
(a new key with a suffix). **Keep mine** is the default when the local
project has `environments` and the imported one does not. *(Builder-check
answer; my call.)*

### Migration of legacy projects

On first start with legacy projects present, the app shows a **"Config
needs updating"** dialog listing, per project, the environments it would
create. The user can untick, rename or change ports, then **Apply**. Before
writing, the current file is copied to
`projects.json.bak-YYYYMMDD-HHMMSS` next to it. "Later" leaves the file
alone, and legacy projects keep working the old way; the dialog is offered
again from a banner in Settings.

The proposal for each legacy project:

1. Take the profiles today's rules match (`get_profiles_for_project` as it
   is now).
2. Group them by account. Profiles with an unknown account are listed as
   "skipped, account unknown" with a hint to add the environment by hand.
3. Port: what `get_local_port` gives that profile today. When several
   profiles share an account, prefer the one whose port comes from an
   `envPortMapping` match. Those profiles also become remembered
   candidates for that account.
4. **Pre-ticked** only if the port comes from an `envPortMapping` match. An
   environment that would only fall back to `defaultPort` is shown unticked.
   That is how `covered-replica` ends up not migrated without any special
   case.
5. Name: the profile name with the `profileFilter` prefix and the separators
   that follow it removed (`covered-staging` → `staging`). If that leaves
   nothing, use `prod`, unless another environment in the project is
   already `prod`, in which case use the full profile name.

For the owner's config this proposes prod/staging/dev with 5460/5461/5462
(DB) and 6080/6081/6082 (VNC), with `covered-replica` unticked.

### Project form in Settings

- **Every `ProjectConfig` field is editable.** Rarely used ones go under an
  "Advanced" disclosure: `secretPath`, `secretUsernameField`,
  `secretPasswordField`, `multiplexed`, `bastionPattern`.
- **Environments section.** A table of name, account, port and an optional
  override row (`rdsPattern`, `region`). The account field is a dropdown of
  accounts found in local profiles, labelled with the profile names, and
  also accepts a typed 12-digit ID. Each row shows how it resolves: the
  profile, "pick profile", or "no profile → Create profile".
  - **Copy environments from…** fills the table from another project (names,
    accounts, and ports).
  - **Add environment** suggests the next free port after the highest in
    the table, or `defaultPort` when the table is empty.
- **Duplicate project** in the project list, for adding the next database of
  the same system.
- **Inline validation.** The form calls the backend validator as fields
  change (debounced) and shows errors next to the field. Save is disabled
  while the form is invalid. There is one set of rules, in Rust.
- **Refresh after save.** After any save in Settings (project, AWS profile,
  migration) the main connection form reloads projects **and** the
  environments of the selected project. Today only the project list is
  refreshed.

### Authentication in the form

- An RDS PostgreSQL project gets an **Authentication** select:
  - **Automatic** (the default for new projects): the hint reads "Uses an
    IAM token or the password, whichever the database asks for";
  - **IAM only**;
  - **Password only**.
- For MySQL the choices are **IAM** and **Password**.
- Meaning:
  - **Automatic** finds the secret (prefix, then the
    `rds/<id>-credentials` fallback from #64, or the exact `secretPath`).
    The DB user is the secret's username. The #64 proxy picks the credential
    per sign-in.
    - **"No secret"** means the project has **no secret configured**: empty
      `secretPrefix` and no `secretPath`. Only then is "IAM DB User" used.
    - A configured secret that is not found, or is denied, **fails the
      connect with that error, as on #64**. It never falls through to
      another DB user. *(Builder-check answer; my call, from the
      no-behaviour-change anti-goal.)*
  - **IAM only** never reads a secret.
  - **Password only** never mints an IAM token. For PostgreSQL this is a new
    password-only upstream mode in the #64 proxy, next to `IamOnly` and
    `Auto`. The DB client keeps the same local password and loopback TLS as
    every other PostgreSQL project. *(Builder-check answer; my call: one way
    of connecting for all PostgreSQL projects.)*
- **On disk:**

  | Form choice | `authType` | `passwordOnly` |
  |---|---|---|
  | Automatic | `secrets` | false/absent |
  | IAM only | `iam` | — |
  | Password only | `secrets` | `true` |

  On #64 a PostgreSQL `secrets` project already behaves automatically, so
  existing projects keep their behaviour with no rewrite.
- The connection card keeps showing the method actually used (#64 already
  adds this).

## Constraints

- Svelte 5 runes, with the existing patterns in `CLAUDE.md` and the project
  memory: `$props()` destructuring, no `_` prefixes, the a11y rules for
  dialogs, the colour-contrast values, and no `transition: all`.
- Rust edition 2024. `cargo check --no-default-features` must keep building
  the CLI, so account resolution, migration and validation live in `config/`
  with no Tauri dependency.
- macOS sandbox path handling: read `~/.aws/config` only through
  `aws_config.rs` (`AWS_CONFIG_FILE`).
- No new heavy dependencies.
- No JS test runner exists. Frontend behaviour is covered by a written
  manual checklist in the PR.
- **Mac App Store work** (`feat/mac-app-store`: sandboxed build, demo mode
  with `src/lib/demo.js` mocks and a test that the mocked shapes match the
  UI). Whichever lands second updates the demo mocks for the new commands
  and data shapes. If 3.8 ships in the sandboxed build, verify these work
  under sandbox paths: "Create profile" (writing `~/.aws/config`), the
  backup files, the atomic-write temp file, and
  `~/.connection-app/profile-accounts.json` / `preferences.json`.
  *(Builder-check answer; my call.)*

## Tests the builder must provide

- Account extraction: `sso_account_id`, `sso_session` profiles, `role_arn`
  in standard, China and GovCloud partitions, unknown.
- Resolution: 0, 1 and several candidates, the remembered choice, a cache
  hit and invalidation.
- Migration on a fixture shaped like the owner's config (fake account IDs):
  prod/staging/dev, the pre-tick rule, naming collisions, unknown accounts,
  a second profile for the same account.
- Per-project loading: one broken project among good ones, a whole-file
  syntax error, broken entries surviving a save byte-for-byte, atomic write.
- Validation of the new fields.
- Saved-connection conversion, including the failure case.
- CLI: `--env`, `--profile` belonging to an environment versus not.
- The account check at connect refuses a mismatch (unit-level, with the STS
  call abstracted).

- 3.7.x compatibility: a project created in 3.8 (with the sentinel filter)
  and a migrated project both deserialize with the 3.7.x `ProjectConfig`
  rules.
- Import: key collisions, a backup is written, and a legacy file from a
  3.7.x teammate does not overwrite a migrated project by default.

## Left to the builder

- Re-keying of data stored per `project:profile` (local proxy passwords,
  bastion preferences, secret refetch) to `project:environment`, with
  migration of the existing entries.
- When a saved connection's remembered profile and the per-account choice
  disagree, which one wins.
- Tie-breaks in migration beyond the rules above.

- Exact Tauri command names and signatures, and the store and cache file
  formats.
- Visual layout of the environments table, the migration dialog and the
  broken-project states, within the existing Settings style.
- How the debounced validation is wired.
- Whether the profile picker sits inline or in a popover.
