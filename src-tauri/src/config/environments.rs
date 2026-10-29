use crate::config::accounts::ProfileAccount;
use crate::config::preferences::{Preferences, get_account_profile};
use crate::config::projects::{
    Environment, ProjectConfig, get_local_port, get_profiles_for_project,
};
use crate::error::AppError;
use serde::Serialize;
use std::collections::HashMap;

/// An environment of a project together with the AWS profile that reaches it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedEnvironment {
    pub environment: Environment,
    pub legacy: bool,
    pub profile: Option<String>,
    pub candidates: Vec<String>,
}

/// Resolve a project's environments to profiles.
///
/// Account-based projects match profiles by account; the remembered choice
/// wins when it is still a candidate, otherwise the first candidate in config
/// order. Legacy projects yield one profile-named environment per profile.
pub fn resolve_environments(
    project: &ProjectConfig,
    all_projects: &HashMap<String, ProjectConfig>,
    profiles: &[ProfileAccount],
    prefs: &Preferences,
) -> Vec<ResolvedEnvironment> {
    if project.is_legacy() {
        let names: Vec<String> = profiles.iter().map(|p| p.profile.clone()).collect();
        return get_profiles_for_project(&names, project, all_projects)
            .into_iter()
            .map(|profile| {
                let account_id = profiles
                    .iter()
                    .find(|p| p.profile == profile)
                    .and_then(|p| p.account_id.clone())
                    .unwrap_or_default();
                ResolvedEnvironment {
                    environment: Environment {
                        name: profile.clone(),
                        account_id,
                        port: get_local_port(&profile, project),
                        rds_pattern: None,
                        region: None,
                        target_pattern: None,
                        ecs_cluster: None,
                        ecs_service: None,
                        bastion_pattern: None,
                    },
                    legacy: true,
                    profile: Some(profile.clone()),
                    candidates: vec![profile],
                }
            })
            .collect();
    }

    project
        .environments
        .iter()
        .flatten()
        .map(|env| {
            let candidates: Vec<String> = profiles
                .iter()
                .filter(|p| p.account_id.as_deref() == Some(env.account_id.as_str()))
                .map(|p| p.profile.clone())
                .collect();
            let profile = get_account_profile(prefs, &env.account_id)
                .filter(|r| candidates.iter().any(|c| c == r))
                .map(str::to_string)
                .or_else(|| candidates.first().cloned());
            ResolvedEnvironment {
                environment: env.clone(),
                legacy: false,
                profile,
                candidates,
            }
        })
        .collect()
}

/// Exact, case-sensitive lookup by environment name.
pub fn find_environment<'a>(
    resolved: &'a [ResolvedEnvironment],
    name: &str,
) -> Option<&'a ResolvedEnvironment> {
    resolved.iter().find(|r| r.environment.name == name)
}

/// The project config a connection to `resolved` uses: the project itself for
/// a legacy environment, otherwise the project with the environment's overrides.
pub fn effective_project_config(
    project: &ProjectConfig,
    resolved: &ResolvedEnvironment,
) -> ProjectConfig {
    if resolved.legacy {
        project.clone()
    } else {
        project.for_environment(&resolved.environment)
    }
}

/// The account the caller must be signed in to before connecting, if it is
/// checked: legacy environments are named after profiles and carry no
/// authoritative account.
pub fn account_to_verify(resolved: &ResolvedEnvironment) -> Option<&str> {
    if resolved.legacy {
        None
    } else {
        Some(resolved.environment.account_id.as_str())
    }
}

/// The profile to connect with: `profile_override` when it reaches the
/// environment's account, otherwise the resolved profile.
pub fn pick_profile(
    resolved: &ResolvedEnvironment,
    profile_override: Option<&str>,
) -> Result<String, AppError> {
    let env = &resolved.environment;
    if let Some(p) = profile_override {
        if !resolved.candidates.iter().any(|c| c == p) {
            return Err(AppError::Config(format!(
                "Profile '{}' is not a profile for account {} (environment '{}')",
                p, env.account_id, env.name
            )));
        }
        return Ok(p.to_string());
    }
    resolved.profile.clone().ok_or_else(|| {
        AppError::Config(format!(
            "No AWS profile for account {} (environment '{}')",
            env.account_id, env.name
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::preferences::set_account_profile;
    use serde_json::json;

    const ACCT: &str = "231030592327";
    const SSO: &str = "d-9067000000-AdministratorAccess";

    fn pa(profile: &str, account: Option<&str>) -> ProfileAccount {
        ProfileAccount {
            profile: profile.into(),
            account_id: account.map(String::from),
        }
    }

    fn project(extra: serde_json::Value) -> ProjectConfig {
        let mut v = json!({
            "name": "P", "region": "us-east-1", "profileFilter": null,
            "defaultPort": "5432"
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    }

    fn env_project(envs: serde_json::Value) -> ProjectConfig {
        project(json!({ "environments": envs }))
    }

    fn one_env(account: &str) -> ProjectConfig {
        env_project(json!([{ "name": "prod", "accountId": account, "port": "5460" }]))
    }

    fn resolve(
        p: &ProjectConfig,
        profiles: &[ProfileAccount],
        prefs: &Preferences,
    ) -> Vec<ResolvedEnvironment> {
        resolve_environments(p, &HashMap::new(), profiles, prefs)
    }

    #[test]
    fn one_candidate_is_used() {
        let r = resolve(
            &one_env(ACCT),
            &[pa("covered", Some(ACCT)), pa("other", Some("1"))],
            &Preferences::default(),
        );
        assert_eq!(r[0].profile.as_deref(), Some("covered"));
        assert_eq!(r[0].candidates, vec!["covered"]);
        assert!(!r[0].legacy);
    }

    #[test]
    fn several_candidates_first_in_config_order_without_choice() {
        let profiles = [pa("covered", Some(ACCT)), pa(SSO, Some(ACCT))];
        let r = resolve(&one_env(ACCT), &profiles, &Preferences::default());
        assert_eq!(r[0].profile.as_deref(), Some("covered"));
        assert_eq!(r[0].candidates, vec!["covered", SSO]);
    }

    #[test]
    fn remembered_choice_wins() {
        let profiles = [pa("covered", Some(ACCT)), pa(SSO, Some(ACCT))];
        let mut prefs = Preferences::default();
        set_account_profile(&mut prefs, ACCT, SSO);
        let r = resolve(&one_env(ACCT), &profiles, &prefs);
        assert_eq!(r[0].profile.as_deref(), Some(SSO));
    }

    #[test]
    fn stale_remembered_choice_is_ignored() {
        let profiles = [pa("covered", Some("999999999999")), pa(SSO, Some(ACCT))];
        let mut prefs = Preferences::default();
        set_account_profile(&mut prefs, ACCT, "covered");
        let r = resolve(&one_env(ACCT), &profiles, &prefs);
        assert_eq!(r[0].candidates, vec![SSO]);
        assert_eq!(r[0].profile.as_deref(), Some(SSO));
    }

    #[test]
    fn no_candidate_gives_none() {
        let r = resolve(
            &one_env("000000000000"),
            &[pa("covered", Some(ACCT)), pa("x", None)],
            &Preferences::default(),
        );
        assert_eq!(r[0].profile, None);
        assert!(r[0].candidates.is_empty());
    }

    #[test]
    fn two_envs_same_account_stay_separate() {
        let p = env_project(json!([
            { "name": "blue", "accountId": ACCT, "port": "5460", "rdsPattern": "db-blue" },
            { "name": "green", "accountId": ACCT, "port": "5461", "rdsPattern": "db-green" }
        ]));
        let r = resolve(&p, &[pa("p", Some(ACCT))], &Preferences::default());
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].environment.name, "blue");
        assert_eq!(r[1].environment.name, "green");
        assert!(r.iter().all(|e| e.profile.as_deref() == Some("p")));
        assert_eq!(
            find_environment(&r, "green").unwrap().environment.port,
            "5461"
        );
        assert!(find_environment(&r, "Green").is_none());
    }

    #[test]
    fn legacy_project_yields_profile_named_environments() {
        let p = project(json!({
            "profileFilter": "covered",
            "envPortMapping": { "covered": "5460", "covered-dev": "5462" }
        }));
        let profiles = [
            pa("covered", Some(ACCT)),
            pa("covered-dev", None),
            pa("covered-replica", Some("111111111111")),
            pa("unrelated", None),
        ];
        let r = resolve(&p, &profiles, &Preferences::default());
        let names: Vec<&str> = r.iter().map(|e| e.environment.name.as_str()).collect();
        assert_eq!(names, ["covered", "covered-dev", "covered-replica"]);
        let ports: Vec<&str> = r.iter().map(|e| e.environment.port.as_str()).collect();
        assert_eq!(ports, ["5460", "5462", "5432"]);
        assert!(r.iter().all(|e| e.legacy && e.candidates.len() == 1));
        assert_eq!(r[0].environment.account_id, ACCT);
        assert_eq!(r[1].environment.account_id, "");
        assert_eq!(r[1].profile.as_deref(), Some("covered-dev"));
    }

    #[test]
    fn effective_config_uses_environment_overrides() {
        let p = env_project(json!([
            { "name": "blue", "accountId": ACCT, "port": "5460", "rdsPattern": "db-blue" },
            { "name": "green", "accountId": ACCT, "port": "5461", "rdsPattern": "db-green",
              "region": "eu-west-1" }
        ]));
        let r = resolve(&p, &[pa("p", Some(ACCT))], &Preferences::default());
        let blue = effective_project_config(&p, find_environment(&r, "blue").unwrap());
        let green = effective_project_config(&p, find_environment(&r, "green").unwrap());
        assert_eq!(blue.rds_pattern, "db-blue");
        assert_eq!(blue.region, "us-east-1");
        assert_eq!(green.rds_pattern, "db-green");
        assert_eq!(green.region, "eu-west-1");
    }

    #[test]
    fn effective_config_of_a_legacy_environment_is_the_project() {
        let p = project(json!({ "rdsPattern": "covered-db", "profileFilter": "covered" }));
        let r = resolve(&p, &[pa("covered", Some(ACCT))], &Preferences::default());
        let c = effective_project_config(&p, &r[0]);
        assert_eq!(c.rds_pattern, "covered-db");
        assert_eq!(c.region, p.region);
    }

    #[test]
    fn profile_override_must_be_a_candidate() {
        let profiles = [pa("covered", Some(ACCT)), pa(SSO, Some(ACCT)), pa("other", Some("1"))];
        let r = resolve(&one_env(ACCT), &profiles, &Preferences::default());
        assert_eq!(pick_profile(&r[0], Some(SSO)).unwrap(), SSO);
        let err = pick_profile(&r[0], Some("other")).unwrap_err();
        assert!(matches!(err, AppError::Config(_)), "{err:?}");
        assert!(err.to_string().contains("other"), "{err}");
    }

    #[test]
    fn without_override_the_resolved_profile_is_used() {
        let profiles = [pa("covered", Some(ACCT)), pa(SSO, Some(ACCT))];
        let r = resolve(&one_env(ACCT), &profiles, &Preferences::default());
        assert_eq!(pick_profile(&r[0], None).unwrap(), "covered");
    }

    #[test]
    fn no_profile_for_the_account_is_a_config_error() {
        let r = resolve(&one_env("000000000000"), &[pa("covered", Some(ACCT))], &Preferences::default());
        let err = pick_profile(&r[0], None).unwrap_err();
        assert!(matches!(err, AppError::Config(_)), "{err:?}");
        assert!(
            err.to_string()
                .contains("No AWS profile for account 000000000000 (environment 'prod')"),
            "{err}"
        );
    }

    #[test]
    fn account_to_verify_is_none_for_legacy() {
        let p = project(json!({}));
        let r = resolve(&p, &[pa("dev", Some(ACCT))], &Preferences::default());
        assert!(r[0].legacy);
        assert_eq!(account_to_verify(&r[0]), None);
    }

    #[test]
    fn account_to_verify_is_the_environment_account() {
        let r = resolve(&one_env(ACCT), &[pa("covered", Some(ACCT))], &Preferences::default());
        assert_eq!(account_to_verify(&r[0]), Some(ACCT));
    }
}
