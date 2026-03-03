use crate::config::aws_config::parse_aws_config;

use aws_sdk_ec2 as ec2;
use aws_sdk_rds as rds;
use aws_sdk_secretsmanager as secretsmanager;
use aws_sdk_ssm as ssm;
use aws_sdk_sts as sts;

/// All AWS service clients for a given profile+region.
pub struct AwsClients {
    pub sts: sts::Client,
    pub ec2: ec2::Client,
    pub rds: rds::Client,
    pub ssm: ssm::Client,
    pub secrets_manager: secretsmanager::Client,
}

/// SSO configuration extracted from an AWS profile.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SsoConfig {
    pub start_url: String,
    pub region: String,
    pub account_id: Option<String>,
    pub role_name: Option<String>,
}

/// Authentication type detected from profile config (used by Phase 3 CLI).
#[derive(Debug)]
#[allow(dead_code)]
pub enum AuthType {
    Sso {
        sso_start_url: Option<String>,
        sso_session: Option<String>,
    },
    AssumeRoleMfa,
    AssumeRole,
    Process,
    Static,
    Unknown,
}

/// Build AWS SDK config for a given profile and region.
/// Uses the aws-config crate which handles the full credential chain
/// including SSO, assume-role, process credentials, etc.
pub async fn build_aws_config(profile: &str, region: &str) -> aws_config::SdkConfig {
    let loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .profile_name(profile);

    loader.load().await
}

/// Create all AWS clients for a profile+region.
pub async fn create_aws_clients(profile: &str, region: &str) -> AwsClients {
    let config = build_aws_config(profile, region).await;

    AwsClients {
        sts: sts::Client::new(&config),
        ec2: ec2::Client::new(&config),
        rds: rds::Client::new(&config),
        ssm: ssm::Client::new(&config),
        secrets_manager: secretsmanager::Client::new(&config),
    }
}

/// Detect the authentication type for a profile (used by Phase 3 CLI).
#[allow(dead_code)]
pub async fn detect_auth_type(profile: &str) -> AuthType {
    let profiles = parse_aws_config().await;
    let config = match profiles.get(profile) {
        Some(c) => c,
        None => return AuthType::Unknown,
    };

    if config.contains_key("sso_start_url") || config.contains_key("sso_session") {
        return AuthType::Sso {
            sso_start_url: config.get("sso_start_url").cloned(),
            sso_session: config.get("sso_session").cloned(),
        };
    }

    if config.contains_key("role_arn") && config.contains_key("mfa_serial") {
        return AuthType::AssumeRoleMfa;
    }

    if config.contains_key("role_arn") {
        return AuthType::AssumeRole;
    }

    if config.contains_key("credential_process") {
        return AuthType::Process;
    }

    if config.contains_key("aws_access_key_id") {
        return AuthType::Static;
    }

    AuthType::Unknown
}

/// Extract SSO config for a profile. Handles both legacy SSO (keys on profile)
/// and new-style sso-session sections. Returns None for non-SSO profiles.
pub async fn get_sso_config(profile: &str) -> Option<SsoConfig> {
    let profiles = parse_aws_config().await;
    let config = profiles.get(profile)?;

    let mut start_url = config.get("sso_start_url").cloned();
    let mut region = config.get("sso_region").cloned();
    let account_id = config.get("sso_account_id").cloned();
    let role_name = config.get("sso_role_name").cloned();

    // New-style: profile references an [sso-session <name>] section
    if start_url.is_none() {
        if let Some(sso_session) = config.get("sso_session") {
            let session_key = format!("sso-session {}", sso_session);
            if let Some(session_config) = profiles.get(&session_key) {
                start_url = session_config.get("sso_start_url").cloned();
                if region.is_none() {
                    region = session_config.get("sso_region").cloned();
                }
            }
        }
    }

    let start_url = start_url?;
    let region = region?;

    Some(SsoConfig {
        start_url,
        region,
        account_id,
        role_name,
    })
}

/// Check if any SSO-related config exists for a profile (used by Phase 3 CLI).
#[allow(dead_code)]
pub async fn is_sso_profile(profile: &str) -> bool {
    get_sso_config(profile).await.is_some()
}

