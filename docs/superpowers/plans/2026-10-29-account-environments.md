# Account-based Environments & Config Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Identify environments by AWS account ID instead of profile name, load and repair `projects.json` per project, edit every project field in the app, and offer Automatic / IAM only / Password only database auth.

**Architecture:** The config layer (`src-tauri/src/config/`, no Tauri dependency, so the CLI keeps building) gains:
- an `Environment` type;
- account extraction from AWS profiles;
- environment resolution;
- a per-project file loader with atomic writes and backups;
- migration from the legacy profile-name rules.

Legacy projects resolve to synthetic environments named after the profile, so connect, the CLI and the UI have **one** environment-based path. The tunnel manager connects by environment name, applies per-environment overrides through `ProjectConfig::for_environment`, and checks the caller account before opening the tunnel. The Svelte UI switches from profiles to environments and gets an environments editor, a migration dialog and broken-project states.

**Tech Stack:** Rust 2024, Tauri 2, aws-sdk (sts), serde_json (`raw_value` feature), Svelte 5 runes, Biome.

**Spec:** `docs/superpowers/specs/2026-10-29-account-environments-and-config-repair-design.md`. Read it before any task. The plan argues from it.

**Baseline:** branch `feat/account-environments`, cut from `feat/iam-auth-proxy` (PR #64). If #64 has been merged to `main` by the time you start, rebase onto `main` first.

## Global Constraints

- `cargo check --no-default-features` must build. Everything in `config/` and `aws/` stays free of Tauri types.
- `cargo test`, `cargo clippy --all-targets -- -D warnings` (both with and without default features), `npx @biomejs/biome check src/` and `npm run build:vite` are green at the end of every task.
- Read `~/.aws/config` only through `config/aws_config.rs`, which respects `AWS_CONFIG_FILE`.
- No new crates. Enable the `raw_value` feature on the existing `serde_json` dependency.
- Svelte 5 runes only. No `_`-prefixed names shared between script and template. Dialogs: `role="dialog"`, `tabindex="-1"`, `use:trapFocus`. Non-interactive text uses `#9e9ea7`. Never `transition: all`. Every `listen()` is unlistened in `onDestroy`.
- A file written by 3.8 must deserialize with 3.7.x rules: always write `defaultPort` and `envPortMapping` (`{}` for new projects), and `authType` only as `"secrets"` or `"iam"`.
- New projects get `profileFilter: "__connection-app-3.8__"` (`HIDE_FROM_LEGACY_FILTER`).
- `accountId` is exactly 12 ASCII digits, stored as a string.
- Backups are named `projects.json.bak-YYYYMMDD-HHMMSS`, in the same directory as `projects.json`.
- Account → profile choices live in `~/.connection-app/preferences.json`. Detected accounts live in `~/.connection-app/profile-accounts.json`. Neither file is ever exported.
- `env_logger`/`log` messages never contain passwords, tokens or secret values.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **A remembered profile that now points at another account** (the user edited the profile). The remembered choice must be ignored, not used. Test in Task 3.
2. **Two environments in one project with the same `accountId`** (they differ by `rdsPattern`). Resolution and connect must keep them apart. Tests in Tasks 3 and 9.
3. **`projects.json` missing, `{}`, a top-level array, or 0 bytes.** Expected: missing, `{}` and 0 bytes mean "no projects"; an array is a file-level error. Nothing is overwritten. Test in Task 4.
4. **The same port in different projects** (all four Covered DB projects use 5460). This is valid and must not be "repaired"; uniqueness is only within a project. Test in Task 5.
5. **The local proxy password a DB client already saved** must stay the same after migration renames the environment from `covered-dev` to `dev`. Test in Task 7.

---

### Task 1: Config format: `Environment`, new fields, 3.7.x compatibility

**Files:**
- Modify: `src-tauri/Cargo.toml` (`serde_json = { version = "1", features = ["raw_value"] }`)
- Modify: `src-tauri/src/config/projects.rs`
- Test: the `mod tests` in `src-tauri/src/config/projects.rs`

**Interfaces:**
- Produces:
  - `pub struct Environment { pub name: String, pub account_id: String /* "accountId" */, pub port: String, pub rds_pattern: Option<String>, pub region: Option<String>, pub target_pattern: Option<String>, pub ecs_cluster: Option<String>, pub ecs_service: Option<String>, pub bastion_pattern: Option<String> }`, with camelCase serde names. Every `Option` is `#[serde(default, skip_serializing_if = "Option::is_none")]`. Derives `Serialize, Deserialize, Debug, Clone, PartialEq`.
  - New `ProjectConfig` fields:
    - `pub environments: Option<Vec<Environment>>` (`default`, `skip_serializing_if = "Option::is_none"`);
    - `pub password_only: Option<bool>` (`"passwordOnly"`, same attributes).
  - `env_port_mapping` and `default_port` gain `#[serde(default)]` for reading. They are still always serialized.
  - `pub const HIDE_FROM_LEGACY_FILTER: &str = "__connection-app-3.8__";`
  - `impl ProjectConfig`:
    - `pub fn is_legacy(&self) -> bool`: true when `environments` is `None` or empty;
    - `pub fn for_environment(&self, env: &Environment) -> ProjectConfig`: a clone with each `Some` override applied over the matching project field (`rds_pattern`, `region`, `target_pattern`, `ecs_cluster`, `ecs_service`, `bastion_pattern`);
    - `pub fn auth_mode(&self) -> AuthMode`.
  - `pub enum AuthMode { Auto, IamOnly, PasswordOnly }`:
    - `authType == "iam"` → `IamOnly`;
    - `password_only == Some(true)` → `PasswordOnly`;
    - otherwise `Auto`.

- [ ] **Step 1: Write the failing tests**

```rust
// A copy of the 3.7.10 struct's required/typed fields, test-only.
#[derive(Deserialize)]
struct Legacy37 {
    name: String, region: String,
    #[serde(rename = "envPortMapping")] env_port_mapping: HashMap<String, String>,
    #[serde(rename = "defaultPort")] default_port: String,
    #[serde(rename = "authType", default)] auth_type: String,
    #[serde(rename = "profileFilter")] profile_filter: Option<String>,
}

#[test]
fn new_project_serializes_readable_by_3_7() {
    let mut c = test_config(None);
    c.profile_filter = Some(HIDE_FROM_LEGACY_FILTER.into());
    c.env_port_mapping = HashMap::new();
    c.environments = Some(vec![env("dev", "073813807522", "5462")]);
    c.password_only = Some(true);
    let json = serde_json::to_string(&c).unwrap();
    let old: Legacy37 = serde_json::from_str(&json).unwrap();
    assert!(old.env_port_mapping.is_empty());
    assert_eq!(old.auth_type, "secrets");
    assert_eq!(old.profile_filter.as_deref(), Some(HIDE_FROM_LEGACY_FILTER));
    assert!(json.contains("\"accountId\":\"073813807522\""));
}

#[test]
fn missing_env_port_mapping_and_default_port_still_load() { /* JSON without both → Ok, empty map, "" */ }

#[test]
fn for_environment_applies_only_set_overrides() {
    // project rds_pattern "covered-db", region "us-west-1"; env overrides rds_pattern "covered-db-2"
    // → result rds_pattern "covered-db-2", region "us-west-1", bastion_pattern unchanged
}

#[test]
fn auth_mode_mapping() {
    // ("secrets", None) → Auto; ("secrets", Some(false)) → Auto; ("secrets", Some(true)) → PasswordOnly; ("iam", Some(true)) → IamOnly
}

#[test]
fn is_legacy() { /* None → true, Some(vec![]) → true, Some([env]) → false */ }
```

- [ ] **Step 2: Run them and confirm they fail**

Run: `cd src-tauri && cargo test config::projects`
Expected: compile errors (`Environment`, `HIDE_FROM_LEGACY_FILTER`, `for_environment` and `auth_mode` are not defined).

- [ ] **Step 3: Implement the types and methods listed under Interfaces**

Update every `ProjectConfig { .. }` literal in the crate to include the two new fields: tests, `cli.rs`, and `manager.rs` tests (`cargo check --all-targets` lists them).

- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cd src-tauri && cargo test && cargo check --no-default-features`
Expected: all pass.

- [ ] **Step 5: Commit** `feat(config): Environment type, passwordOnly, 3.7-compatible serialization`

---

### Task 2: Account of an AWS profile, and the detected-account cache

**Files:**
- Create: `src-tauri/src/config/accounts.rs`, registered in `config/mod.rs`
- Modify: `src-tauri/src/aws/operations.rs` (two functions)
- Test: the `mod tests` in `accounts.rs`

**Interfaces:**
- Consumes: `aws_config::{AwsProfile, read_aws_config}`.
- Produces (`config/accounts.rs`):
  - `pub fn account_from_profile(p: &AwsProfile) -> Option<String>`:
    - `sso_account_id`, if it is 12 digits;
    - else the account in `role_arn`, matched by `^arn:aws[a-z-]*:iam::(\d{12}):`;
    - else `None`.
  - `pub fn profile_fingerprint(p: &AwsProfile) -> String`: lowercase hex SHA-256 of `raw_content` (`sha2` is already a dependency).
  - `#[derive(Serialize, Deserialize, Default)] pub struct AccountCache { pub entries: HashMap<String, CachedAccount> }` and `pub struct CachedAccount { pub account_id: String, pub fingerprint: String }`.
  - `pub async fn load_account_cache() -> AccountCache` and `pub async fn save_account_cache(c: &AccountCache)`. Path: `~/.connection-app/profile-accounts.json`. Read errors give the default.
  - `#[derive(Serialize, Clone, Debug, PartialEq)] pub struct ProfileAccount { pub profile: String, pub account_id: Option<String> }`, serialized camelCase.
  - `pub fn profile_accounts(profiles: &[AwsProfile], cache: &AccountCache) -> Vec<ProfileAccount>`. Order is `~/.aws/config` order. A profile that is not derivable falls back to the cache entry, but only if the fingerprint matches.
- Produces (`aws/operations.rs`):
  - `pub async fn caller_account_id(clients: &AwsClients) -> Result<String, AppError>`: `sts:GetCallerIdentity` with the existing 15 s timeout;
  - `pub async fn detect_profile_account(profile: &str, region: &str) -> Result<String, AppError>`: builds clients with `create_aws_clients`, calls `caller_account_id`, and writes the cache entry.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn sso_account_id_wins() { /* sso_account_id "073813807522" + role_arn of another account → "073813807522" */ }
#[test] fn sso_session_style_profile() { /* sso_session set, sso_account_id "231030592327", no sso_start_url → Some */ }
#[test] fn role_arn_partitions() {
    for arn in ["arn:aws:iam::111122223333:role/R", "arn:aws-cn:iam::111122223333:role/R", "arn:aws-us-gov:iam::111122223333:role/R"] { /* → Some("111122223333") */ }
}
#[test] fn unknown_account() { /* static-key profile, malformed sso_account_id "12345" → None */ }
#[test] fn cache_used_only_when_fingerprint_matches() {
    // cache has profile "keys" → "444455556666" with fingerprint of raw "A"; profile raw "A" → Some; raw "B" → None
}
#[test] fn profile_accounts_keeps_config_order() { /* 3 profiles in → same order out */ }
```

- [ ] **Step 2: Run** `cargo test config::accounts`. Expected: fail to compile.
- [ ] **Step 3: Implement the functions listed under Interfaces.** Mirror `preferences.rs` for file I/O. Writes go through `write_atomic` from Task 4. Until Task 4 lands, use a plain write marked `// TODO(task4)`, and remove it in Task 4.
- [ ] **Step 4: Run** `cargo test && cargo check --no-default-features`. Expected: pass.
- [ ] **Step 5: Commit** `feat(config): read the AWS account of each profile`

---

### Task 3: Environment resolution and per-account profile choice

**Files:**
- Create: `src-tauri/src/config/environments.rs`
- Modify: `src-tauri/src/config/preferences.rs` (new field and accessors)
- Test: the `mod tests` in `environments.rs` and `preferences.rs`

**Interfaces:**
- Consumes: `Environment`, `ProjectConfig::is_legacy`, `get_profiles_for_project`, `get_local_port` (Task 1 and existing code); `ProfileAccount` (Task 2).
- Produces (`preferences.rs`):
  - `Preferences.account_profiles: HashMap<String, String>`: account → profile, `#[serde(default)]`;
  - `pub fn get_account_profile<'a>(p: &'a Preferences, account_id: &str) -> Option<&'a str>`;
  - `pub fn set_account_profile(p: &mut Preferences, account_id: &str, profile: &str)`.
- Produces (`environments.rs`):
  - `#[derive(Serialize, Clone, Debug, PartialEq)] #[serde(rename_all = "camelCase")] pub struct ResolvedEnvironment { pub environment: Environment, pub legacy: bool, pub profile: Option<String>, pub candidates: Vec<String> }`
  - `pub fn resolve_environments(project: &ProjectConfig, all_projects: &HashMap<String, ProjectConfig>, profiles: &[ProfileAccount], prefs: &Preferences) -> Vec<ResolvedEnvironment>`
  - `pub fn find_environment<'a>(resolved: &'a [ResolvedEnvironment], name: &str) -> Option<&'a ResolvedEnvironment>`: exact, case-sensitive match.
- Rules:
  - **Non-legacy.**
    - `candidates` = profiles whose `account_id == env.account_id`, in config order.
    - `profile` = the remembered profile if it is among the candidates, else the first candidate, else `None`.
  - **Legacy.** One entry per profile from `get_profiles_for_project`:
    - `environment = Environment { name: <profile>, account_id: <account or "">, port: get_local_port(profile, project), ..None }`;
    - `legacy: true`;
    - `profile: Some(profile)`;
    - `candidates: vec![profile]`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn one_candidate_is_used() {}
#[test] fn several_candidates_first_in_config_order_without_choice() {
    // profiles: covered(231030592327), d-9067…-AdministratorAccess(231030592327) → profile "covered", candidates both
}
#[test] fn remembered_choice_wins() { /* prefs 231030592327 → d-9067… → profile d-9067… */ }
#[test] fn stale_remembered_choice_is_ignored() {
    // Review Focus 1: prefs 231030592327 → "covered", but profile "covered" now has account 999999999999
    // → candidates == [d-9067…], profile == Some(d-9067…)
}
#[test] fn no_candidate_gives_none() { /* env account 000000000000 → profile None, candidates [] */ }
#[test] fn two_envs_same_account_stay_separate() {
    // Review Focus 2: envs "blue"(acct A, rdsPattern "db-blue") and "green"(acct A, rdsPattern "db-green")
    // → 2 resolved entries, names blue/green, both profile Some(p)
}
#[test] fn legacy_project_yields_profile_named_environments() {
    // profileFilter "covered", envPortMapping {covered:5460, covered-dev:5462}, profiles covered, covered-dev, covered-replica
    // → names [covered, covered-dev, covered-replica], ports [5460, 5462, defaultPort], all legacy
}
```

- [ ] **Step 2: Run** `cargo test config::environments config::preferences`. Expected: fail to compile.
- [ ] **Step 3: Implement the functions listed under Interfaces.**
- [ ] **Step 4: Run** `cargo test`. Expected: pass.
- [ ] **Step 5: Commit** `feat(config): resolve environments to AWS profiles by account`

---

### Task 4: Per-project loading, atomic writes, backups

**Files:**
- Create: `src-tauri/src/config/project_file.rs`
- Modify: `src-tauri/src/config/projects.rs`. `load_project_configs`, `save_project_config` and `delete_project_config` become thin wrappers over `project_file` with the same signatures, so existing callers are unchanged.
- Modify: `config/accounts.rs` (switch to `write_atomic`)
- Test: the `mod tests` in `project_file.rs`, using `tempfile`-free temp dirs under `std::env::temp_dir()` with a unique subdirectory per test

**Interfaces:**
- Produces:
  - `pub enum ProjectEntry { Valid { config: ProjectConfig, problems: Vec<FieldError> }, Broken { raw: Box<serde_json::value::RawValue>, error: String } }`. `FieldError` comes from Task 5. Until then `problems` is `Vec::new()`, filled in Task 5.
  - `pub struct ProjectFile { pub entries: BTreeMap<String, ProjectEntry> }`.
  - `#[derive(Serialize, Debug)] #[serde(tag = "kind", rename_all = "camelCase")] pub enum FileError { Syntax { line: usize, column: usize, message: String }, NotAnObject, Io { message: String } }`.
  - `pub async fn load_project_file_at(path: &Path) -> Result<ProjectFile, FileError>`:
    - missing, 0 bytes or only whitespace → empty;
    - top-level not an object → `NotAnObject`;
    - otherwise parse into `BTreeMap<String, Box<RawValue>>`, then each value: `normalize` it, then deserialize it.
  - `pub async fn load_project_file() -> Result<ProjectFile, FileError>`: the same, at the default path.
  - `fn normalize(v: &mut serde_json::Value) -> Vec<String>`. Safe fixes only, and returns a description of each:
    - JSON numbers to strings for `defaultPort`, `envPortMapping` values and `environments[].port`;
    - trims surrounding whitespace on every string field.
  - `impl ProjectFile`:
    - `pub fn valid_configs(&self) -> HashMap<String, ProjectConfig>`;
    - `pub fn upsert(&mut self, key: &str, config: ProjectConfig)`;
    - `pub fn upsert_raw(&mut self, key: &str, json: &str) -> Result<(), String>`: must parse as a JSON object;
    - `pub fn remove(&mut self, key: &str)`;
    - `pub fn to_json(&self) -> String`: pretty-printed, trailing newline. `Broken` entries are emitted from `raw` verbatim.
  - `pub async fn write_atomic(path: &Path, contents: &str) -> Result<(), AppError>`: writes `<name>.tmp-<pid>` in the same directory, then `rename`.
  - `pub async fn write_project_file_at(path: &Path, file: &ProjectFile) -> Result<(), AppError>`, plus the default-path wrapper `write_project_file`.
  - `pub async fn backup_project_file_at(path: &Path) -> Result<Option<PathBuf>, AppError>`: copies to `projects.json.bak-YYYYMMDD-HHMMSS`, or returns `None` if the file is missing. Use local time from `std::time::SystemTime`. Formatting the timestamp by hand is fine; no chrono crate.
  - `pub async fn list_backups_at(path: &Path) -> Vec<PathBuf>`: newest first.
  - `pub async fn read_raw_at(path: &Path) -> Result<String, AppError>`.
  - `pub async fn write_raw_at(path: &Path, text: &str) -> Result<(), FileError>`: refuses to write unless `text` parses as a JSON object, then writes atomically.
- **Never** write the file as a side effect of loading.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test] async fn one_broken_project_does_not_hide_others() {
    // {"good": <valid>, "bad": {"name": 5}} → good Valid, bad Broken with error mentioning "name"
}
#[tokio::test] async fn broken_entry_survives_save_verbatim() {
    // load, upsert("good", changed), write; the file text still contains the exact original "bad" JSON substring
}
#[tokio::test] async fn empty_missing_and_whitespace_files_are_empty() {}                  // Review Focus 3
#[tokio::test] async fn top_level_array_is_not_an_object_and_file_untouched() {}           // Review Focus 3
#[tokio::test] async fn syntax_error_reports_line_and_column_and_file_untouched() {
    // "{\n  \"a\": {,\n}" → Syntax { line: 2, .. }; file bytes unchanged
}
#[tokio::test] async fn numeric_ports_are_normalized() { /* "defaultPort": 5432 → "5432", Valid */ }
#[tokio::test] async fn atomic_write_leaves_no_tmp_file() {}
#[tokio::test] async fn backup_name_and_listing() { /* name matches ^projects\.json\.bak-\d{8}-\d{6}$, newest first */ }
#[tokio::test] async fn write_raw_refuses_invalid_json() {}
```

- [ ] **Step 2: Run** `cargo test config::project_file`. Expected: fail to compile.
- [ ] **Step 3: Implement the functions listed under Interfaces,** and rewire `projects.rs` and `accounts.rs`.
- [ ] **Step 4: Run** `cargo test && cargo check --no-default-features`. Expected: pass, including all existing `projects.rs` tests.
- [ ] **Step 5: Commit** `feat(config): load projects.json per project with atomic writes and backups`

---

### Task 5: Validation with field paths and the new rules

**Files:**
- Modify: `src-tauri/src/config/validation.rs`
- Modify: callers of `ValidationResult.errors` (`commands/projects.rs`, `commands/system.rs`, `cli.rs` if used)
- Modify: `project_file.rs` (fill `problems`)

**Interfaces:**
- Produces:
  - `#[derive(Serialize, Clone, Debug, PartialEq)] pub struct FieldError { pub field: String, pub message: String }`.
    - `field` is the camelCase JSON path, e.g. `"region"`, `"environments[1].accountId"`.
    - Use `""` for project-level errors.
  - `pub struct ValidationResult { pub valid: bool, pub errors: Vec<FieldError> }`, with `pub fn messages(&self) -> Vec<String>` formatting `"<field>: <message>"`.
- New rules (existing rules keep their messages, now with a `field`):
  - environments present → each `name` non-empty and `SHELL_SAFE_PATTERN`, names unique within the project;
  - `accountId` matches `^\d{12}$`;
  - `port` matches `PORT_PATTERN`, and ports are unique within the project;
  - override fields use the same patterns as their project-level fields;
  - `passwordOnly == Some(true)` requires `authType != "iam"`;
  - for a non-legacy project, `envPortMapping` and `profileFilter` are not validated.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn bad_account_id_points_at_field() { /* "07381380752" → field "environments[0].accountId" */ }
#[test] fn duplicate_env_names_and_ports_within_project() {}
#[test] fn same_port_in_two_projects_is_valid() {}   // Review Focus 4: validate each of two configs using 5460 → both valid
#[test] fn password_only_with_iam_is_invalid() {}
#[test] fn env_name_with_space_is_rejected() {}
#[test] fn existing_rules_carry_field_names() { /* missing region → field "region" */ }
```

- [ ] **Step 2: Run** `cargo test config::validation`. Expected: fail.
- [ ] **Step 3: Implement the rules listed under Interfaces,** update the callers, and have `project_file` put `validate_project_config(..).errors` into `ProjectEntry::Valid.problems`.
- [ ] **Step 4: Run** `cargo test`. Expected: pass.
- [ ] **Step 5: Commit** `feat(config): field-level validation for environments and auth`

---

### Task 6: Migration proposal and apply

**Files:**
- Create: `src-tauri/src/config/migration.rs`
- Test: the `mod tests` in `migration.rs`

**Interfaces:**
- Consumes: `ProjectFile` (Task 4), `ProfileAccount` (Task 2), `get_profiles_for_project`, `get_local_port`, `Preferences` + `set_account_profile` (Task 3).
- Produces:
  - `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] #[serde(rename_all = "camelCase")] pub struct ProposedEnvironment { pub name: String, pub account_id: String, pub port: String, pub profiles: Vec<String>, pub preferred_profile: String, pub selected: bool }`
  - `pub struct SkippedProfile { pub profile: String, pub reason: String }`
  - `pub struct ProjectProposal { pub project_key: String, pub project_name: String, pub environments: Vec<ProposedEnvironment>, pub skipped: Vec<SkippedProfile> }`
  - `pub fn propose_migration(projects: &HashMap<String, ProjectConfig>, profiles: &[ProfileAccount]) -> Vec<ProjectProposal>`: legacy projects only, sorted by key.
  - `#[derive(Deserialize)] pub struct MigrationChoice { pub project_key: String, pub environments: Vec<ProposedEnvironment> }`: what the user confirmed, possibly edited.
  - `pub fn apply_migration(file: &mut ProjectFile, choices: &[MigrationChoice], prefs: &mut Preferences)`:
    - for each choice with ≥1 `selected` environment, set `environments` from the selected ones;
    - leave every legacy field untouched;
    - when `profiles.len() > 1`, call `set_account_profile(account, preferred_profile)`.
- Proposal rules (spec, "Migration of legacy projects"):
  - group matched profiles by account; an unknown account → `skipped`, with reason `"account unknown"`;
  - `port` = `get_local_port(preferred_profile)`. `preferred_profile` = the first profile in the group whose port comes from an `envPortMapping` match, else the first profile;
  - `selected` = whether that port came from an `envPortMapping` match;
  - `name` = the profile name with the `profileFilter` prefix removed, then leading `-`, `_` and `.` trimmed. If empty → `"prod"`, unless `"prod"` is already taken in this project, in which case the full profile name.

- [ ] **Step 1: Write the failing tests** (fixture shaped like the owner's config, using fake account IDs 1111…, 2222…, 3333…, 4444…)

```rust
#[test] fn owner_shaped_config_proposes_prod_staging_dev() {
    // profiles: covered(111111111111), covered-staging(222222222222), covered-dev(333333333333),
    //           covered-replica(444444444444), d-9067-111111111111-Admin(111111111111)
    // project "covered": filter "covered", mapping {covered:5460, covered-staging:5461, covered-dev:5462}, defaultPort 5432
    // → [prod 111… 5460 selected, staging 222… 5461 selected, dev 333… 5462 selected, replica 444… 5432 NOT selected]
    // d-9067… is not matched by the "covered" filter, so prod.profiles == ["covered"]
}
#[test] fn same_account_two_profiles_prefers_mapped_one() {}
#[test] fn prod_name_collision_uses_full_profile_name() {}
#[test] fn unknown_account_is_skipped_with_reason() {}
#[test] fn apply_keeps_legacy_fields_and_records_preferred_profile() {}
#[test] fn apply_with_nothing_selected_leaves_project_legacy() {}
#[test] fn vnc_project_gets_its_own_ports() { /* mapping {covered:6080,…} → 6080/6081/6082 */ }
```

- [ ] **Step 2: Run** `cargo test config::migration`. Expected: fail.
- [ ] **Step 3: Implement the proposal and apply rules above.**
- [ ] **Step 4: Run** `cargo test`. Expected: pass.
- [ ] **Step 5: Commit** `feat(config): migrate profile-name projects to account environments`

---

### Task 7: Re-key per-profile stores to per-environment

**Files:**
- Modify: `src-tauri/src/config/local_passwords.rs`, `src-tauri/src/config/preferences.rs`
- Test: existing test modules in both files

**Interfaces:**
- Produces:
  - `pub async fn local_proxy_password(project_key: &str, environment: &str, legacy_profiles: &[String]) -> Result<String, AppError>`:
    - lookup key `"{project}:{environment}"`;
    - if absent, and some `"{project}:{p}"` exists for `p` in `legacy_profiles` (in order), **copy** that password to the new key (the old key stays) and return it;
    - else create a new one.
  - `get_preferred_bastion(prefs, project_key, environment, legacy_profiles: &[String]) -> Option<&str>`: the same fallback, read-only. `set_preferred_bastion(prefs, project_key, environment, instance_id)` always writes the environment key.
- For legacy environments the name equals the profile, so the key is unchanged.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test] async fn migrated_environment_keeps_the_saved_client_password() {
    // Review Focus 5: file has "covered:covered-dev" = "abc…" ; call (covered, "dev", ["covered-dev"]) → "abc…",
    // and the file now also has "covered:dev" = "abc…"
}
#[tokio::test] async fn legacy_key_unchanged() { /* (covered, "covered-dev", ["covered-dev"]) → same key, no new entry */ }
#[test] fn bastion_preference_falls_back_to_profile_key() {}
```

- [ ] **Step 2: Run** `cargo test config::local_passwords config::preferences`. Expected: fail.
- [ ] **Step 3: Implement the fallback,** then update the callers in `manager.rs` and `cli.rs`: pass `env.environment.name` and `env.candidates`.
- [ ] **Step 4: Run** `cargo test`. Expected: pass.
- [ ] **Step 5: Commit** `refactor(config): key local passwords and bastion choices by environment`

---

### Task 8: Password-only upstream mode in the PostgreSQL proxy

**Files:**
- Modify: `src-tauri/src/tunnel/pg_proxy/mod.rs`
- Modify: `src-tauri/src/tunnel/manager.rs` (around `UpstreamAuth::` at lines ~701 and ~736) and `src-tauri/src/cli.rs` (~309 and ~340): choose the variant from `project_config.auth_mode()`
- Test: `src-tauri/src/tunnel/pg_proxy/tests.rs`

**Interfaces:**
- Produces: `UpstreamAuth::PasswordOnly { password: String, refetch: Option<SecretRefetch> }`. It behaves like `Auto`, except it never asks `TokenSource` for a token:
  - when RDS asks for cleartext or PAM, the proxy answers with the **password**;
  - when RDS rejects it, the client gets the RDS error (sqlstate kept).
- Mapping at the call sites:
  - `AuthMode::IamOnly` → `IamOnly`;
  - `Auto` → `Auto` (unchanged);
  - `PasswordOnly` → `PasswordOnly`.
- MySQL keeps today's path; for MySQL, `PasswordOnly` behaves like `secrets`.

- [ ] **Step 1: Write the failing tests**, reusing the existing fake RDS harness in `tests.rs` (`Rds::Scram`, `Rds::Cleartext`/PAM)

```rust
#[tokio::test] async fn password_only_never_mints_a_token() {
    // TokenSource fake that panics/counts; RDS asks SCRAM → signs in with password, token count 0
}
#[tokio::test] async fn password_only_sends_password_when_rds_asks_cleartext() {
    // RDS cleartext expecting "secret-pw" → success, token count 0
}
#[tokio::test] async fn auto_behaviour_unchanged() { /* existing Auto tests still pass untouched */ }
```

- [ ] **Step 2: Run** `cargo test pg_proxy`. Expected: fail to compile.
- [ ] **Step 3: Implement the variant and the call-site mapping.**
- [ ] **Step 4: Run** `cargo test`. Expected: pass, and every existing `pg_proxy` test is unchanged.
- [ ] **Step 5: Commit** `feat(pg-proxy): password-only upstream mode`

---

### Task 9: Connect by environment, with the account check

**Files:**
- Modify: `src-tauri/src/tunnel/manager.rs` (`connect` at ~282 and `ActiveConnection`/`ConnectionInfo`)
- Modify: `src-tauri/src/commands/connection.rs` (`connect` command)
- Modify: `src-tauri/src/aws/operations.rs` (pure helper)
- Test: `manager.rs` tests and `operations.rs` tests

**Interfaces:**
- Consumes:
  - `resolve_environments` and `find_environment` (Task 3);
  - `ProjectConfig::for_environment` (Task 1);
  - `caller_account_id` (Task 2);
  - `profile_accounts`, `load_account_cache`, `read_aws_config`, `load_preferences`.
- Produces:
  - `pub fn ensure_account(expected: &str, actual: &str, environment: &str) -> Result<(), AppError>`, in `operations.rs`. On mismatch it returns `AppError::Aws("Profile signed in to account <actual>, but environment '<environment>' is account <expected>. Pick another profile in Settings.")`.
  - `TunnelManager::connect(&self, project_key: &str, environment: &str, profile_override: Option<&str>, local_port: Option<&str>, database: Option<&str>, used_ports: &[String])`, same return type.
    - It resolves the environment.
    - `profile_override` must be in `candidates`, else `AppError::Config`.
    - The profile is `override.or(resolved.profile)`. `None` → `AppError::Config("No AWS profile for account <id> (environment '<name>')")`.
    - The port is `local_port.or(env.port)`.
    - It uses `project_config.for_environment(&env)` everywhere `project_config` was used.
    - For non-legacy environments it calls `ensure_account(env.account_id, caller_account_id(&clients)?, name)` right after `create_aws_clients`, before any tunnel work.
    - `rediscover_target` on auto-reconnect uses the same effective config.
  - `ConnectionInfo` and `ActiveConnection` gain `pub environment: String`, serialized as `"environment"`. `profile` stays.
  - Tauri `connect(project_key, environment, profile: Option<String>, local_port, database, saved_connection_id)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn ensure_account_mismatch_message_names_both_accounts() {}
#[test] fn ensure_account_match_ok() {}
#[test] fn effective_config_uses_environment_overrides() {
    // Review Focus 2: two envs, same account, rdsPattern "db-blue"/"db-green" → manager's effective config
    // (extract `fn effective_project_config(project, resolved) -> ProjectConfig` as a pure helper and test it)
}
#[test] fn profile_override_must_be_a_candidate() { /* pure helper `fn pick_profile(resolved, override) -> Result<String, AppError>` */ }
```

- [ ] **Step 2: Run** `cargo test`. Expected: fail to compile.
- [ ] **Step 3: Implement the connect changes listed under Interfaces.** Keep the pure helpers (`effective_project_config`, `pick_profile`) in `config/environments.rs` so the CLI can use them.
- [ ] **Step 4: Run** `cargo test && cargo clippy --all-targets -- -D warnings`. Expected: pass.
- [ ] **Step 5: Commit** `feat(tunnel): connect by environment and verify the AWS account`

---

### Task 10: Tauri commands for projects, environments, repair, migration, import, saved connections

**Files:**
- Modify: `src-tauri/src/commands/projects.rs`, `profiles.rs`, `system.rs`, `saved.rs`; register commands in `src-tauri/src/lib.rs`
- Test: unit tests for the pure parts (DTO building, saved-connection conversion, import plan)

**Interfaces** (all names camelCase on the JS side):
- `list_projects() -> ProjectsOverview`:
  - `ProjectsOverview { projects: Vec<ProjectSummary>, file_error: Option<FileError>, needs_migration: bool, backups: Vec<String> }`
  - `ProjectSummary { key, name, connection_type, databases, environments: Vec<ResolvedEnvironment>, problems: Vec<FieldError>, broken: Option<String>, reachable: bool }`
  - `reachable` = at least one environment has `profile.is_some()`.
- `list_aws_accounts() -> Vec<ProfileAccount>`.
- `set_account_profile(account_id, profile)` and `detect_profile_account(profile) -> String`. The latter uses the region of the profile, or `us-east-1`.
- `aws_profile_template(account_id) -> AwsProfile`: `sso_start_url`, `sso_region` and `sso_role_name` copied from the first SSO profile in config, `sso_account_id = account_id`, name `""`.
- `validate_project(config) -> Vec<FieldError>`.
- `list_project_entries() -> Vec<ProjectEntryDto { key, config: Option<ProjectConfig>, raw: Option<String>, error: Option<String>, problems: Vec<FieldError> }>`. This replaces `list_project_configs`; delete that command.
- `save_project_config(key, config)`: validates first, then `ProjectFile::upsert` + write.
- `save_project_raw(key, json)`.
- `delete_project_config(key)`.
- `get_project_file_raw() -> String`.
- `save_project_file_raw(text)`.
- `restore_project_backup(path)`: path must be one of `list_backups`, and the current file is backed up first.
- `get_migration_proposal() -> Vec<ProjectProposal>` and `apply_migration(choices: Vec<MigrationChoice>)`: backup → apply → write → save preferences.
- Import becomes two steps, replacing `import_projects_file`:
  - `pick_import_file() -> ImportPreview { path, items: Vec<ImportItem { key, status: "new" | "conflict" | "broken", local_migrated: bool, imported_legacy: bool, default_choice: "replace" | "keepMine" | "copy", error: Option<String> }> }`;
  - `apply_import(path, choices: HashMap<String, String>) -> usize`: backup first; `copy` → key `<key>-imported`, or `-imported-2`, … until the key is free; broken items are skipped.
  - `export_projects_file` is unchanged.
- `list_profiles` is deleted. The frontend uses `list_projects().environments` (Task 12).
- Saved connections:
  - `SavedConnection` gains `environment: Option<String>`.
  - `load_saved_connections` returns `SavedConnectionDto { ..SavedConnection, needs_attention: bool }`. For entries without `environment`, it fills it from the resolved environments of that project: the environment whose `candidates` contain `profile`, else the one whose `account_id` equals the profile's account.
  - `needs_attention = true` when nothing matches. Converted entries are written back; unmatched ones are kept as they are.
  - `save_connection` / `update_saved_connection` take `environment`.

- [ ] **Step 1: Write the failing tests** for the pure helpers

```rust
#[test] fn saved_connection_maps_profile_to_environment() {}
#[test] fn saved_connection_without_match_needs_attention_and_is_kept() {}
#[test] fn import_default_keeps_migrated_local_over_legacy_import() {}
#[test] fn import_copy_key_is_unique() { /* "covered" exists, "covered-imported" exists → "covered-imported-2" */ }
#[test] fn reachable_requires_a_resolved_profile() {}
```

- [ ] **Step 2: Run** `cargo test commands`. Expected: fail.
- [ ] **Step 3: Implement the commands listed under Interfaces.**
- [ ] **Step 4: Run** `cargo test && cargo clippy --all-targets -- -D warnings`. Expected: pass. `npm run build:vite` may fail until Task 12; that is expected only between Tasks 10 and 12, so do Tasks 10–12 in one sitting.
- [ ] **Step 5: Commit** `feat(commands): environments, repair, migration and import commands`

---

### Task 11: CLI `--env`

**Files:**
- Modify: `src-tauri/src/cli.rs`

**Interfaces:**
- `--env <NAME>`: exact match through `find_environment`. When it does not match, the error is `Unknown environment 'X' for <project>. Available: prod, staging, dev`.
- `--profile` must be a candidate of the chosen environment. With no `--env`, it selects the environment whose candidates contain it.
- Interactive list items read `prod — covered (231030592327)`. Environments with no profile are listed as disabled, with the reason.
- `projects` subcommand: per project, print environments with account, port and resolved profile, or `no profile`.
- Legacy projects: migrate in memory only. Print once: `Note: projects.json uses the old profile-name format. Open ConnectionApp to update it.`
- The CLI never writes `projects.json`.

- [ ] **Step 1: Write the failing tests:** extract `fn select_environment(resolved: &[ResolvedEnvironment], env: Option<&str>, profile: Option<&str>) -> Result<&ResolvedEnvironment, String>` and test it:

```rust
#[test] fn env_flag_exact_match() {}
#[test] fn unknown_env_lists_available_names() { /* "Prod" vs "prod" → Err containing "Available: prod" */ }
#[test] fn profile_flag_selects_its_environment() {}
#[test] fn profile_not_in_env_is_error() {}
```

- [ ] **Step 2: Run** `cargo test --no-default-features --bin connection-app-cli`. Expected: fail.
- [ ] **Step 3: Implement the flag and the printing changes listed under Interfaces.**
- [ ] **Step 4: Run** `cargo test --no-default-features && cargo clippy --no-default-features --all-targets -- -D warnings`. Expected: pass.
- [ ] **Step 5: Commit** `feat(cli): --env and account-based environments`

---

### Task 12: Connection form, App wiring and saved connections

**Files:**
- Modify: `src/App.svelte`, `src/lib/ConnectionForm.svelte`, `src/lib/SavedConnections.svelte`

**Interfaces:**
- Consumes: `list_projects` → `ProjectsOverview`; `connect` with `environment`; `set_account_profile`; saved-connection DTO fields `environment` and `needsAttention` (Task 10).
- ConnectionForm props:
  - `projects` (the `ProjectSummary[]`);
  - `selectedProject`, `selectedEnvironment`, `profileOverride` (bindable);
  - `onCreateProfile(accountId)`, which opens Settings → AWS profile editor pre-filled from `aws_profile_template`.
- Behaviour:
  - **Project select** lists `reachable` projects. Broken or invalid projects appear greyed out with `broken`, or the first `problems` message, as the option's label suffix. Unreachable projects are not listed.
  - **Environment select** lists `environments[].environment.name`. An environment with `profile == null` is shown disabled, as `dev — no AWS profile for 0738…7522`, with a "Create profile" button beside the select when it is chosen.
  - **"via profile ▾" select** appears only when `candidates.length > 1`. Changing it calls `set_account_profile` and sets `profileOverride`.
  - **Refresh after save.** `refreshProjects()` reloads `list_projects` and keeps the selection if it still exists. Settings calls it after **every** save: project, raw project, whole file, AWS profile, migration, import, restore.
  - **Saved connections** show `project / environment`. `needsAttention` entries show a warning badge and "Edit", which opens the existing saved-connection edit with the environment select.
  - **Active connection card** shows the environment, and the profile in secondary text.

- [ ] **Step 1: Implement** the props and behaviour listed under Interfaces.
- [ ] **Step 2: Run** `npx @biomejs/biome check src/ && npm run build:vite`. Expected: no errors.
- [ ] **Step 3: Manual check** in `npm run dev:gui` against the owner-shaped config. Add each line to the PR checklist:
  - environments show prod/staging/dev;
  - `covered-replica` is not shown after migration;
  - the "via profile" select appears for prod only if both profiles match;
  - a disabled environment offers "Create profile".
- [ ] **Step 4: Commit** `feat(ui): pick environments instead of AWS profiles`

---

### Task 13: Settings: full project form, environments editor, broken projects, file errors

**Files:**
- Create: `src/lib/EnvironmentsEditor.svelte`
- Modify: `src/lib/Settings.svelte`

**Interfaces:**
- `EnvironmentsEditor` props:
  - `environments` (bindable array of `Environment`);
  - `accounts` (`ProfileAccount[]` from `list_aws_accounts`);
  - `projects` (for "Copy from…");
  - `connectionType`;
  - `defaultPort`;
  - `errors` (`FieldError[]` filtered to `environments[...]`).
- Environments editor:
  - **Rows:** name, account and port. Account is an `<input list>` backed by a `<datalist>` of accounts labelled with profile names, and also accepts a typed ID.
  - **Overrides** sit in an expandable row: `rdsPattern` and `region` for RDS; `targetPattern`, `ecsCluster`, `ecsService` and `bastionPattern` for services.
  - **Resolution status per row:** the profile name, "pick profile", or "no profile → Create profile".
  - **Add environment** suggests `max(port)+1`, or `defaultPort` when there are no environments.
  - **Copy from…** copies another project's environments.
- Project form in `Settings.svelte`:
  - **Removed:** the `profileFilter` and `envPortMapping` inputs. They are hidden for non-legacy projects. A legacy project shows a notice with "Update to environments", which opens the migration dialog (Task 14).
  - **Advanced disclosure:** `secretPath`, `secretUsernameField`, `secretPasswordField`, `multiplexed` (checkbox, default on) and `bastionPattern`.
  - **Authentication select:**
    - PostgreSQL: Automatic / IAM only / Password only, with the hint "Uses an IAM token or the password, whichever the database asks for" under Automatic;
    - MySQL: IAM / Password.
    - On save: Automatic → `authType: "secrets", passwordOnly: null`; IAM only → `"iam", null`; Password only → `"secrets", true`.
  - **Validation:** call `validate_project` 300 ms after the last change. Show each `FieldError` next to its field. Disable Save while there are errors.
  - **New projects** are saved with `profileFilter: "__connection-app-3.8__"` and `envPortMapping: {}`.
- Project list:
  - **Duplicate** action: opens the form pre-filled from the project, with key `<key>-copy` and name `<name> (copy)`;
  - **Broken entries:** a red badge plus Edit (form pre-filled from what parsed, when `config` is present), Edit JSON (a textarea for the single project, saved via `save_project_raw`) and Delete;
  - **Invalid entries:** an amber badge listing `problems`.
  - **Projects with `reachable == false`:** a grey "No AWS access" badge. They are only shown here, never in the connection form.
- **File error banner** (`fileError` from `list_projects`): shows `Syntax` line/column, a whole-file raw editor (`get_project_file_raw` / `save_project_file_raw`), and Restore backup (a select of `backups`, then `restore_project_backup`) behind `ConfirmDialog`.

- [ ] **Step 1: Implement `EnvironmentsEditor.svelte`** as specified under Interfaces.
- [ ] **Step 2: Rework the project form and list in `Settings.svelte`** as specified under Interfaces.
- [ ] **Step 3: Run** `npx @biomejs/biome check src/ && npm run build:vite`. Expected: no errors and no Svelte a11y warnings.
- [ ] **Step 4: Manual check**, added to the PR checklist:
  - corrupt one project by hand → the others still connect, and the broken one shows Edit JSON;
  - break the whole file → banner, fix it in the raw editor;
  - restore a backup;
  - Automatic → Password only → IAM only round-trips correctly in `projects.json`;
  - a new project created in the app appears in the connection form without restarting.
- [ ] **Step 5: Commit** `feat(settings): full project form with environments editor and repair actions`

---

### Task 14: Migration dialog and import-conflict dialog

**Files:**
- Create: `src/lib/MigrationDialog.svelte`, `src/lib/ImportDialog.svelte`
- Modify: `src/App.svelte` (open the migration dialog on start when `needsMigration`), `src/lib/Settings.svelte` (banner + import button)

**Interfaces:**
- `MigrationDialog` props: `proposals` (`ProjectProposal[]`), `onApply(choices)`, `onLater()`.
  - Per project: a table of proposed environments with a checkbox (`selected`), an editable name and port, and the account (read-only).
  - `skipped` is listed below, with its reason.
  - Apply → `apply_migration`, then `refreshProjects()`.
  - Later → closes. Settings shows a "Config needs updating" banner while `needsMigration`.
  - The start-up prompt appears once per app launch at most.
- `ImportDialog` props: `preview` (`ImportPreview`), `onApply(choices)`, `onCancel()`. One row per item:
  - **new:** imported;
  - **conflict:** a Replace / Keep mine / Import as copy radio, defaulting to `defaultChoice`;
  - **broken:** shown with its error and skipped.

- [ ] **Step 1: Implement both dialogs** with the existing `ConfirmDialog`/edit-modal styles and a11y rules.
- [ ] **Step 2: Run** `npx @biomejs/biome check src/ && npm run build:vite`. Expected: no errors.
- [ ] **Step 3: Manual check** with a copy of the owner's real `projects.json` in a scratch `HOME`. Add to the PR checklist:
  - the dialog proposes prod/staging/dev on 5460/5461/5462 and 6080/6081/6082, with `covered-replica` unticked;
  - after Apply a backup exists and the legacy fields are unchanged;
  - a 3.7.10 build still lists the projects from the migrated file;
  - importing the pre-migration file defaults to "Keep mine".
- [ ] **Step 4: Commit** `feat(ui): migration and import dialogs`

---

### Task 15: Docs, version and release notes

**Files:**
- Modify: `README.md` (projects.json reference: `environments`, `passwordOnly`, migration, the `AWS_SSM_PLUGIN_ADDITIONAL_STREAM_HOSTS` mention stays)
- Modify: `CLAUDE.md` (Core Flow steps 3–4 now say "environments by AWS account"; new `config/` modules listed)
- Modify: `CHANGELOG.md` (`## [3.8.0] - <release date>`, sections Added / Changed / Fixed / Notes. Notes must say a migrated file still works in 3.7.x and that new projects are hidden there.)
- Modify: version `3.8.0` in `package.json`, `package-lock.json` (2 places), `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/tauri.conf.json`

- [ ] **Step 1: Make the edits listed above.**
- [ ] **Step 1a: Mac App Store branch.** If `feat/mac-app-store` is already on `main`, update `src/lib/demo.js`:
  - mocks for `list_projects` (the `ProjectsOverview` shape), `list_project_entries`, `list_aws_accounts`, `validate_project`, `get_migration_proposal`, and `connect` with `environment`;
  - make its shape test pass.

  If it is not on `main` yet, add a line to the PR body saying the MAS branch must do this when it rebases.
- [ ] **Step 2: Run** the full gate: `cd src-tauri && cargo test && cargo test --no-default-features && cargo clippy --all-targets -- -D warnings && cargo clippy --no-default-features --all-targets -- -D warnings && cargo audit` then `cd .. && npx @biomejs/biome check src/ && npm run build:vite`. Expected: everything green, and `cargo audit` reports 0 vulnerabilities.
- [ ] **Step 3: Commit** `chore(release): v3.8.0`
- [ ] **Step 4: Open the PR** against `main`. The body lists the manual checklist from Tasks 12–14 with each item ticked or explicitly marked "not done". **Do not tag.** The owner tags after a GUI run.
