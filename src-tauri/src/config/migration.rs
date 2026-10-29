//! Converting legacy (profile-name based) projects into account environments.

use super::accounts::ProfileAccount;
use super::preferences::{set_account_profile, Preferences};
use super::project_file::{ProjectEntry, ProjectFile};
use super::projects::{get_local_port, get_profiles_for_project, mapped_port, Environment, ProjectConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProposedEnvironment {
    pub name: String,
    pub account_id: String,
    pub port: String,
    pub profiles: Vec<String>,
    pub preferred_profile: String,
    pub selected: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkippedProfile {
    pub profile: String,
    pub reason: String,
    /// The profile's account is not known; "Detect account" can find it.
    #[serde(default)]
    pub account_unknown: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectProposal {
    pub project_key: String,
    pub project_name: String,
    pub environments: Vec<ProposedEnvironment>,
    pub skipped: Vec<SkippedProfile>,
}

/// What the user confirmed, possibly edited.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MigrationChoice {
    pub project_key: String,
    pub environments: Vec<ProposedEnvironment>,
}

/// Environment name for a profile: the filter prefix removed, then leading
/// `-`, `_` and `.` trimmed. An empty result is "prod", unless taken.
fn environment_name(profile: &str, filter: Option<&str>, taken: &[String]) -> String {
    let rest = filter
        .and_then(|f| profile.strip_prefix(f))
        .unwrap_or(profile);
    let name = rest.trim_start_matches(['-', '_', '.']);
    if !name.is_empty() {
        name.to_string()
    } else if taken.iter().any(|t| t == "prod") {
        profile.to_string()
    } else {
        "prod".to_string()
    }
}

fn propose_project(
    key: &str,
    config: &ProjectConfig,
    projects: &HashMap<String, ProjectConfig>,
    profiles: &[ProfileAccount],
) -> ProjectProposal {
    let names: Vec<String> = profiles.iter().map(|p| p.profile.clone()).collect();
    let matched = get_profiles_for_project(&names, config, projects);

    // Accounts in order of first appearance, each with its profiles.
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut skipped = Vec::new();
    for profile in matched {
        let account = profiles
            .iter()
            .find(|p| p.profile == profile)
            .and_then(|p| p.account_id.clone());
        match account {
            None => skipped.push(SkippedProfile {
                profile,
                reason: "account unknown".to_string(),
                account_unknown: true,
            }),
            Some(id) => match groups.iter_mut().find(|(a, _)| *a == id) {
                Some((_, list)) => list.push(profile),
                None => groups.push((id, vec![profile])),
            },
        }
    }

    let mut environments: Vec<ProposedEnvironment> = Vec::new();
    for (account_id, group) in groups {
        let mapped = group
            .iter()
            .find(|p| mapped_port(p, config).is_some())
            .cloned();
        let preferred = mapped.clone().unwrap_or_else(|| group[0].clone());
        let port = get_local_port(&preferred, config);
        // Another mapped profile of the same account loses its own port.
        for other in group.iter().filter(|p| **p != preferred) {
            if let Some(other_port) = mapped_port(other, config)
                && other_port != port
            {
                skipped.push(SkippedProfile {
                    profile: other.clone(),
                    reason: format!(
                        "same account as {}; its port {} is not kept",
                        preferred, other_port
                    ),
                    account_unknown: false,
                });
            }
        }
        let taken: Vec<String> = environments.iter().map(|e| e.name.clone()).collect();
        environments.push(ProposedEnvironment {
            name: environment_name(&preferred, config.profile_filter.as_deref(), &taken),
            account_id,
            port,
            profiles: group,
            preferred_profile: preferred,
            selected: mapped.is_some(),
        });
    }

    ProjectProposal {
        project_key: key.to_string(),
        project_name: config.name.clone(),
        environments,
        skipped,
    }
}

/// One proposal per legacy project, sorted by project key.
pub fn propose_migration(
    projects: &HashMap<String, ProjectConfig>,
    profiles: &[ProfileAccount],
) -> Vec<ProjectProposal> {
    let mut keys: Vec<&String> = projects
        .iter()
        .filter(|(_, c)| c.is_legacy())
        .map(|(k, _)| k)
        .collect();
    keys.sort();
    keys.into_iter()
        .map(|k| propose_project(k, &projects[k], projects, profiles))
        .collect()
}

/// Turn the confirmed environments into account environments. Legacy fields
/// stay as they are so 3.7.x can still read the file.
///
/// `profiles` are all local profiles with their accounts. Whenever an
/// account has more than one of them, the environment's preferred profile is
/// remembered for it, so resolution does not switch to another profile that
/// comes first in `~/.aws/config` (for example one the filter never matched).
pub fn apply_migration(
    file: &mut ProjectFile,
    choices: &[MigrationChoice],
    profiles: &[ProfileAccount],
    prefs: &mut Preferences,
) {
    for choice in choices {
        let Some(ProjectEntry::Valid { config, .. }) = file.entries.get(&choice.project_key)
        else {
            continue;
        };
        let selected: Vec<&ProposedEnvironment> =
            choice.environments.iter().filter(|e| e.selected).collect();
        if selected.is_empty() {
            continue;
        }
        let mut config = config.clone();
        config.environments = Some(
            selected
                .iter()
                .map(|e| Environment {
                    name: e.name.clone(),
                    account_id: e.account_id.clone(),
                    port: e.port.clone(),
                    rds_pattern: None,
                    region: None,
                    target_pattern: None,
                    ecs_cluster: None,
                    ecs_service: None,
                    bastion_pattern: None,
                })
                .collect(),
        );
        for e in &selected {
            let candidates = profiles
                .iter()
                .filter(|p| p.account_id.as_deref() == Some(e.account_id.as_str()))
                .count();
            if candidates > 1 || e.profiles.len() > 1 {
                set_account_profile(prefs, &e.account_id, &e.preferred_profile);
            }
        }
        file.upsert(&choice.project_key, config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::project_file::ProjectEntry;
    use crate::config::preferences::get_account_profile;

    const A: &str = "111111111111";
    const B: &str = "222222222222";
    const C: &str = "333333333333";
    const D: &str = "444444444444";

    fn project(filter: Option<&str>, mapping: &[(&str, &str)]) -> ProjectConfig {
        let map: HashMap<&str, &str> = mapping.iter().cloned().collect();
        serde_json::from_value(serde_json::json!({
            "name": "Covered DB",
            "region": "us-west-1",
            "database": "db",
            "rdsType": "instance",
            "rdsPattern": "covered-db",
            "profileFilter": filter,
            "envPortMapping": map,
            "defaultPort": "5432",
        }))
        .unwrap()
    }

    fn pa(profile: &str, account: Option<&str>) -> ProfileAccount {
        ProfileAccount {
            profile: profile.to_string(),
            account_id: account.map(|a| a.to_string()),
        }
    }

    fn owner_profiles() -> Vec<ProfileAccount> {
        vec![
            pa("covered", Some(A)),
            pa("covered-staging", Some(B)),
            pa("covered-dev", Some(C)),
            pa("covered-replica", Some(D)),
            pa("d-9067-111111111111-Admin", Some(A)),
        ]
    }

    fn one(key: &str, cfg: ProjectConfig) -> HashMap<String, ProjectConfig> {
        HashMap::from([(key.to_string(), cfg)])
    }

    fn summary(p: &ProjectProposal) -> Vec<(String, String, String, bool)> {
        p.environments
            .iter()
            .map(|e| (e.name.clone(), e.account_id.clone(), e.port.clone(), e.selected))
            .collect()
    }

    fn s(a: &str, b: &str, c: &str, d: bool) -> (String, String, String, bool) {
        (a.into(), b.into(), c.into(), d)
    }

    #[test]
    fn owner_shaped_config_proposes_prod_staging_dev() {
        let cfg = project(
            Some("covered"),
            &[("covered", "5460"), ("covered-staging", "5461"), ("covered-dev", "5462")],
        );
        let out = propose_migration(&one("covered", cfg), &owner_profiles());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].project_key, "covered");
        assert_eq!(out[0].project_name, "Covered DB");
        assert_eq!(
            summary(&out[0]),
            vec![
                s("prod", A, "5460", true),
                s("staging", B, "5461", true),
                s("dev", C, "5462", true),
                s("replica", D, "5432", false),
            ]
        );
        assert_eq!(out[0].environments[0].profiles, vec!["covered"]);
        assert!(out[0].skipped.is_empty());
    }

    #[test]
    fn same_account_two_profiles_prefers_mapped_one() {
        let cfg = project(Some("covered"), &[("covered-admin", "5460")]);
        let profiles = vec![pa("covered-ro", Some(A)), pa("covered-admin", Some(A))];
        let out = propose_migration(&one("covered", cfg), &profiles);
        let e = &out[0].environments[0];
        assert_eq!(e.profiles, vec!["covered-ro", "covered-admin"]);
        assert_eq!(e.preferred_profile, "covered-admin");
        assert_eq!(e.port, "5460");
        assert!(e.selected);
        assert_eq!(e.name, "admin");
    }

    #[test]
    fn prod_name_collision_uses_full_profile_name() {
        // "covered" -> prod; "covered-" trims to empty too, but prod is taken.
        let cfg = project(Some("covered"), &[]);
        let profiles = vec![pa("covered", Some(A)), pa("covered-", Some(B))];
        let out = propose_migration(&one("covered", cfg), &profiles);
        let names: Vec<_> = out[0].environments.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["prod", "covered-"]);
    }

    #[test]
    fn unknown_account_is_skipped_with_reason() {
        let cfg = project(Some("covered"), &[("covered", "5460")]);
        let profiles = vec![pa("covered", Some(A)), pa("covered-x", None)];
        let out = propose_migration(&one("covered", cfg), &profiles);
        assert_eq!(out[0].environments.len(), 1);
        assert_eq!(
            out[0].skipped,
            vec![SkippedProfile {
                profile: "covered-x".into(),
                reason: "account unknown".into(),
                account_unknown: true,
            }]
        );
    }

    #[test]
    fn non_legacy_projects_are_not_proposed_and_output_is_sorted() {
        let mut modern = project(Some("covered"), &[]);
        modern.environments = Some(vec![super::super::projects::Environment {
            name: "prod".into(),
            account_id: A.into(),
            port: "5432".into(),
            rds_pattern: None,
            region: None,
            target_pattern: None,
            ecs_cluster: None,
            ecs_service: None,
            bastion_pattern: None,
        }]);
        let mut all = HashMap::from([
            ("zeta".to_string(), project(Some("covered"), &[])),
            ("alpha".to_string(), project(Some("covered"), &[])),
        ]);
        all.insert("modern".into(), modern);
        let out = propose_migration(&all, &owner_profiles());
        let keys: Vec<_> = out.iter().map(|p| p.project_key.as_str()).collect();
        assert_eq!(keys, vec!["alpha", "zeta"]);
    }

    fn file_with(key: &str, cfg: ProjectConfig) -> ProjectFile {
        let mut f = ProjectFile::default();
        f.upsert(key, cfg);
        f
    }

    fn config_of(f: &ProjectFile, key: &str) -> ProjectConfig {
        match &f.entries[key] {
            ProjectEntry::Valid { config, .. } => config.clone(),
            ProjectEntry::Broken { .. } => panic!("broken"),
        }
    }

    #[test]
    fn apply_keeps_legacy_fields_and_records_preferred_profile() {
        let cfg = project(Some("covered"), &[("covered-admin", "5460")]);
        let mut file = file_with("covered", cfg.clone());
        let profiles = vec![pa("covered-ro", Some(A)), pa("covered-admin", Some(A)), pa("covered-dev", Some(C))];
        let proposal = propose_migration(&one("covered", cfg.clone()), &profiles);
        let mut envs = proposal[0].environments.clone();
        envs[1].selected = true;
        let choice = MigrationChoice { project_key: "covered".into(), environments: envs };
        let mut prefs = Preferences::default();
        apply_migration(&mut file, &[choice], &profiles, &mut prefs);

        let after = config_of(&file, "covered");
        let envs = after.environments.clone().unwrap();
        assert_eq!(envs.len(), 2);
        assert_eq!(envs[0].name, "admin");
        assert_eq!(envs[0].account_id, A);
        assert_eq!(envs[0].port, "5460");
        assert_eq!(envs[0].rds_pattern, None);
        assert_eq!(envs[0].bastion_pattern, None);
        assert_eq!(after.profile_filter, cfg.profile_filter);
        assert_eq!(after.env_port_mapping, cfg.env_port_mapping);
        assert_eq!(after.default_port, cfg.default_port);
        assert_eq!(get_account_profile(&prefs, A), Some("covered-admin"));
        // single-profile account records nothing
        assert_eq!(get_account_profile(&prefs, C), None);
    }

    #[test]
    fn apply_remembers_profile_when_an_unmatched_profile_shares_the_account() {
        // The SSO default name is not matched by the filter but comes first
        // in config order; without a remembered choice it would win.
        let cfg = project(Some("covered"), &[("covered", "5460")]);
        let mut file = file_with("covered", cfg.clone());
        let profiles = vec![
            pa("d-9067836ed2-111111111111-AdministratorAccess", Some(A)),
            pa("covered", Some(A)),
            pa("covered-dev", Some(C)),
        ];
        let proposal = propose_migration(&one("covered", cfg), &profiles);
        let envs = proposal[0].environments.clone();
        assert_eq!(envs[0].profiles, vec!["covered"]);
        let choice = MigrationChoice { project_key: "covered".into(), environments: envs };
        let mut prefs = Preferences::default();
        apply_migration(&mut file, &[choice], &profiles, &mut prefs);
        assert_eq!(get_account_profile(&prefs, A), Some("covered"));
        assert_eq!(get_account_profile(&prefs, C), None);
    }

    #[test]
    fn mapped_profile_of_the_same_account_with_another_port_is_listed_as_skipped() {
        let cfg = project(Some("covered"), &[("covered", "5460"), ("covered-admin", "5470")]);
        let profiles = vec![pa("covered", Some(A)), pa("covered-admin", Some(A))];
        let out = propose_migration(&one("covered", cfg), &profiles);
        assert_eq!(out[0].environments.len(), 1);
        assert_eq!(out[0].environments[0].port, "5460");
        assert_eq!(out[0].environments[0].preferred_profile, "covered");
        assert_eq!(
            out[0].skipped,
            vec![SkippedProfile {
                profile: "covered-admin".into(),
                reason: "same account as covered; its port 5470 is not kept".into(),
                account_unknown: false,
            }]
        );
    }

    #[test]
    fn same_account_same_port_is_not_skipped() {
        let cfg = project(Some("covered"), &[("covered", "5460"), ("covered-admin", "5460")]);
        let profiles = vec![pa("covered", Some(A)), pa("covered-admin", Some(A))];
        let out = propose_migration(&one("covered", cfg), &profiles);
        assert!(out[0].skipped.is_empty());
    }

    #[test]
    fn apply_with_nothing_selected_leaves_project_legacy() {
        let cfg = project(Some("covered"), &[]);
        let mut file = file_with("covered", cfg.clone());
        let mut proposal = propose_migration(&one("covered", cfg), &owner_profiles());
        for e in &mut proposal[0].environments {
            e.selected = false;
        }
        let choice = MigrationChoice {
            project_key: "covered".into(),
            environments: proposal[0].environments.clone(),
        };
        let mut prefs = Preferences::default();
        apply_migration(&mut file, &[choice], &owner_profiles(), &mut prefs);
        assert!(config_of(&file, "covered").is_legacy());
        assert!(prefs.account_profiles.is_empty());
    }

    #[test]
    fn apply_ignores_unknown_keys() {
        let mut file = file_with("a", project(Some("covered"), &[]));
        let choice = MigrationChoice {
            project_key: "nope".into(),
            environments: vec![ProposedEnvironment {
                name: "prod".into(),
                account_id: A.into(),
                port: "5432".into(),
                profiles: vec!["x".into()],
                preferred_profile: "x".into(),
                selected: true,
            }],
        };
        apply_migration(&mut file, &[choice], &[], &mut Preferences::default());
        assert_eq!(file.entries.len(), 1);
        assert!(config_of(&file, "a").is_legacy());
    }

    #[test]
    fn vnc_project_gets_its_own_ports() {
        let cfg = project(
            Some("covered"),
            &[("covered", "6080"), ("covered-staging", "6081"), ("covered-dev", "6082")],
        );
        let out = propose_migration(&one("covered", cfg), &owner_profiles());
        let ports: Vec<_> = out[0].environments.iter().map(|e| e.port.as_str()).collect();
        assert_eq!(ports, vec!["6080", "6081", "6082", "5432"]);
    }
}
