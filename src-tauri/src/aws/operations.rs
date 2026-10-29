use crate::aws::credentials::AwsClients;
use crate::error::AppError;
use futures_util::future::BoxFuture;

const DEFAULT_CREDENTIAL_TIMEOUT_SECS: u64 = 15;

#[derive(Debug)]
#[allow(dead_code)]
pub struct CredentialCheck {
    pub valid: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DbCredentials {
    pub username: String,
    pub password: String,
    pub database: String,
    pub secret_name: String,
    /// True when no secret matched the prefix and the IaC-named fallback was used.
    pub via_fallback: bool,
}

/// The RDS instance or cluster a connection goes to.
#[derive(Debug, Clone, PartialEq)]
pub struct RdsTarget {
    pub identifier: String,
    pub endpoint: String,
}

/// Secret named by the IaC convention for an RDS instance or cluster:
/// `rds/<DBInstanceIdentifier or DBClusterIdentifier>-credentials`.
pub fn fallback_secret_name(identifier: &str) -> String {
    format!("rds/{}-credentials", identifier)
}

/// The available RDS resource whose identifier equals `pattern`, otherwise
/// the first available one whose identifier contains it, in API order.
/// Items are (identifier, status, endpoint).
fn first_matching_target<'a>(
    items: impl IntoIterator<Item = (&'a str, &'a str, Option<&'a str>)>,
    pattern: &str,
) -> Option<RdsTarget> {
    let available: Vec<_> = items
        .into_iter()
        .filter(|(_, status, _)| *status == "available")
        .collect();
    available
        .iter()
        .find(|(identifier, _, _)| *identifier == pattern)
        .or_else(|| available.iter().find(|(identifier, _, _)| identifier.contains(pattern)))
        .and_then(|(identifier, _, endpoint)| {
            endpoint.map(|endpoint| RdsTarget {
                identifier: identifier.to_string(),
                endpoint: endpoint.to_string(),
            })
        })
}

/// Read access to Secrets Manager, so secret selection can be tested.
pub trait SecretStore: Send + Sync {
    /// Names of secrets whose name starts with `prefix`, in API order.
    fn names_with_prefix<'a>(&'a self, prefix: &'a str) -> BoxFuture<'a, Result<Vec<String>, AppError>>;
    /// The SecretString of the secret named exactly `name`; `None` if no such secret exists.
    fn secret_string<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<String>, AppError>>;
}

impl SecretStore for AwsClients {
    fn names_with_prefix<'a>(&'a self, prefix: &'a str) -> BoxFuture<'a, Result<Vec<String>, AppError>> {
        Box::pin(async move {
            let response = self
                .secrets_manager
                .list_secrets()
                .filters(
                    aws_sdk_secretsmanager::types::Filter::builder()
                        .key(aws_sdk_secretsmanager::types::FilterNameStringType::Name)
                        .values(prefix)
                        .build(),
                )
                .send()
                .await
                .map_err(|e| AppError::Aws(format!("Failed to list secrets: {}", e)))?;
            Ok(response
                .secret_list()
                .iter()
                .filter_map(|s| s.name().map(str::to_string))
                .collect())
        })
    }

    fn secret_string<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<String>, AppError>> {
        Box::pin(async move {
            match self.secrets_manager.get_secret_value().secret_id(name).send().await {
                Ok(response) => response.secret_string().map(|s| Some(s.to_string())).ok_or_else(|| {
                    AppError::Aws(format!("Secret '{}' has no SecretString value.", name))
                }),
                Err(e)
                    if e.as_service_error()
                        .is_some_and(|se| se.is_resource_not_found_exception()) =>
                {
                    Ok(None)
                }
                Err(e) => Err(AppError::Aws(format!("Failed to get secret value: {}", e))),
            }
        })
    }
}

/// Where a project's database secret lives, so it can be read again later
/// (e.g. after the auth proxy sees the password rejected).
#[derive(Debug, Clone)]
pub struct SecretLocator {
    pub profile: String,
    pub region: String,
    pub secret_prefix: String,
    pub database: String,
    pub secret_path: Option<String>,
    pub username_field: Option<String>,
    pub password_field: Option<String>,
    pub fallback_secret: Option<String>,
}

impl SecretLocator {
    pub async fn fetch(&self) -> Result<DbCredentials, AppError> {
        let clients = crate::aws::credentials::create_aws_clients(&self.profile, &self.region).await;
        get_connection_credentials(
            &clients,
            &self.secret_prefix,
            &self.database,
            self.secret_path.as_deref(),
            self.username_field.as_deref(),
            self.password_field.as_deref(),
            self.fallback_secret.as_deref(),
        )
        .await
    }
}

/// Check if AWS credentials are valid via STS GetCallerIdentity.
pub async fn check_credentials_valid(clients: &AwsClients) -> CredentialCheck {
    let timeout = tokio::time::Duration::from_secs(DEFAULT_CREDENTIAL_TIMEOUT_SECS);

    match tokio::time::timeout(timeout, clients.sts.get_caller_identity().send()).await {
        Ok(Ok(_)) => CredentialCheck {
            valid: true,
            error: None,
        },
        Ok(Err(e)) => CredentialCheck {
            valid: false,
            error: Some(e.to_string()),
        },
        Err(_) => CredentialCheck {
            valid: false,
            error: Some("Credential check timed out after 15 seconds".to_string()),
        },
    }
}

/// The AWS account the clients' credentials belong to (`sts:GetCallerIdentity`).
pub async fn caller_account_id(clients: &AwsClients) -> Result<String, AppError> {
    let timeout = tokio::time::Duration::from_secs(DEFAULT_CREDENTIAL_TIMEOUT_SECS);

    match tokio::time::timeout(timeout, clients.sts.get_caller_identity().send()).await {
        Ok(Ok(out)) => out
            .account()
            .map(str::to_string)
            .ok_or_else(|| AppError::Aws("GetCallerIdentity returned no account".to_string())),
        Ok(Err(e)) => Err(AppError::Aws(format!(
            "Could not determine the AWS account: {}",
            e
        ))),
        Err(_) => Err(AppError::Aws(
            "Account lookup timed out after 15 seconds".to_string(),
        )),
    }
}

/// Refuse to connect when the credentials belong to another account than
/// the environment.
pub fn ensure_account(expected: &str, actual: &str, environment: &str) -> Result<(), AppError> {
    if expected == actual {
        return Ok(());
    }
    Err(AppError::Aws(format!(
        "Profile signed in to account {}, but environment '{}' is account {}. Pick another profile in Settings.",
        actual, environment, expected
    )))
}

/// Ask AWS which account `profile` belongs to and remember the answer in the
/// detected-account cache, keyed to the profile's current config. Caching is
/// best-effort: the account is returned even when it cannot be remembered.
pub async fn detect_profile_account(profile: &str, region: &str) -> Result<String, AppError> {
    let clients = crate::aws::credentials::create_aws_clients(profile, region).await;
    let account_id = caller_account_id(&clients).await?;

    match crate::config::aws_config::read_aws_config().await {
        Ok(profiles) => {
            if let Some(p) = profiles.iter().find(|p| p.name == profile) {
                let mut cache = crate::config::accounts::load_account_cache().await;
                cache.entries.insert(
                    profile.to_string(),
                    crate::config::accounts::CachedAccount {
                        account_id: account_id.clone(),
                        fingerprint: crate::config::accounts::profile_fingerprint(p),
                    },
                );
                crate::config::accounts::save_account_cache(&cache).await;
            } else {
                log::warn!("Detected the account of '{}' but it is no longer in the AWS config; not cached", profile);
            }
        }
        Err(e) => log::warn!("Detected the account of '{}' but could not cache it: {}", profile, e),
    }
    Ok(account_id)
}

/// Traverse a serde_json::Value using dot-notation field path.
/// For example, "credentials.username" traverses into {"credentials": {"username": "foo"}}.
fn get_nested_field<'a>(value: &'a serde_json::Value, field_path: &str) -> Option<&'a str> {
    let parts: Vec<&str> = field_path.split('.').collect();
    let mut current = value;
    for part in &parts {
        current = current.get(*part)?;
    }
    current.as_str()
}

/// Get database credentials from Secrets Manager.
///
/// If `secret_path` is provided, it is used directly as the secret ID (bypasses list_secrets).
/// Otherwise the first secret whose name starts with `secret_prefix` is used,
/// and when none does, the secret named exactly `fallback_secret` (see
/// `fallback_secret_name`).
/// `username_field` and `password_field` allow customizing the JSON field names
/// (supports dot-notation for nested fields). Defaults to "username" and "password".
pub async fn get_connection_credentials(
    clients: &AwsClients,
    secret_prefix: &str,
    database: &str,
    secret_path: Option<&str>,
    username_field: Option<&str>,
    password_field: Option<&str>,
    fallback_secret: Option<&str>,
) -> Result<DbCredentials, AppError> {
    fetch_credentials(
        clients,
        secret_prefix,
        database,
        secret_path,
        username_field,
        password_field,
        fallback_secret,
    )
    .await
}

async fn fetch_credentials(
    store: &dyn SecretStore,
    secret_prefix: &str,
    database: &str,
    secret_path: Option<&str>,
    username_field: Option<&str>,
    password_field: Option<&str>,
    fallback_secret: Option<&str>,
) -> Result<DbCredentials, AppError> {
    let username_key = username_field.unwrap_or("username");
    let password_key = password_field.unwrap_or("password");

    let not_found = |name: &str| AppError::Aws(format!("Secret '{}' was not found.", name));
    let (secret_name, secret_string, via_fallback) = if let Some(path) = secret_path {
        // Use secret_path directly — skip list_secrets
        let value = store.secret_string(path).await?.ok_or_else(|| not_found(path))?;
        (path.to_string(), value, false)
    } else if let Some(name) = store.names_with_prefix(secret_prefix).await?.into_iter().next() {
        let value = store.secret_string(&name).await?.ok_or_else(|| not_found(&name))?;
        (name, value, false)
    } else if let Some(fallback) = fallback_secret {
        let value = store.secret_string(fallback).await?.ok_or_else(|| {
            AppError::Aws(format!(
                "No secret found matching prefix '{}', and no secret named '{}'.",
                secret_prefix, fallback
            ))
        })?;
        (fallback.to_string(), value, true)
    } else {
        return Err(AppError::Aws(format!(
            "No secret found matching prefix '{}'.",
            secret_prefix
        )));
    };
    let secret_string = secret_string.as_str();

    let credentials: serde_json::Value = serde_json::from_str(secret_string).map_err(|e| {
        AppError::Aws(format!(
            "Failed to parse credentials from secret '{}': {}",
            secret_name, e
        ))
    })?;

    let username = get_nested_field(&credentials, username_key)
        .ok_or_else(|| {
            AppError::Aws(format!(
                "Secret '{}' is missing required field: {}",
                secret_name, username_key
            ))
        })?
        .to_string();

    let password = get_nested_field(&credentials, password_key)
        .ok_or_else(|| {
            AppError::Aws(format!(
                "Secret '{}' is missing required field: {}",
                secret_name, password_key
            ))
        })?
        .to_string();

    Ok(DbCredentials {
        username,
        password,
        database: database.to_string(),
        secret_name,
        via_fallback,
    })
}

/// Find a running bastion instance tagged with the given Name pattern.
/// If `preferred_id` is provided and that instance exists and is running, it is returned directly.
/// Otherwise, the normal tag-based search is used.
pub async fn find_bastion_instance(
    clients: &AwsClients,
    bastion_pattern: &str,
    preferred_id: Option<&str>,
) -> Result<String, AppError> {
    // If a preferred bastion is specified, check if it's still running
    if let Some(pref_id) = preferred_id {
        let pref_response = clients
            .ec2
            .describe_instances()
            .instance_ids(pref_id)
            .filters(
                aws_sdk_ec2::types::Filter::builder()
                    .name("instance-state-name")
                    .values("running")
                    .build(),
            )
            .send()
            .await;

        if let Ok(resp) = pref_response {
            for reservation in resp.reservations() {
                for instance in reservation.instances() {
                    if let Some(id) = instance.instance_id() {
                        log::info!("Using preferred bastion instance: {}", id);
                        return Ok(id.to_string());
                    }
                }
            }
        }
        log::info!(
            "Preferred bastion {} not available, falling back to tag search",
            pref_id
        );
    }

    let response = clients
        .ec2
        .describe_instances()
        .filters(
            aws_sdk_ec2::types::Filter::builder()
                .name("tag:Name")
                .values(bastion_pattern)
                .build(),
        )
        .filters(
            aws_sdk_ec2::types::Filter::builder()
                .name("instance-state-name")
                .values("running")
                .build(),
        )
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to describe instances: {}", e)))?;

    for reservation in response.reservations() {
        for instance in reservation.instances() {
            if let Some(id) = instance.instance_id() {
                return Ok(id.to_string());
            }
        }
    }

    Err(AppError::Aws(format!(
        "No running bastion instance found with tag Name={}.",
        bastion_pattern
    )))
}

/// Get the RDS endpoint (cluster or instance based on rdsType).
pub async fn get_rds_endpoint(
    clients: &AwsClients,
    rds_type: &str,
    rds_pattern: &str,
) -> Result<Option<String>, AppError> {
    Ok(get_rds_target(clients, rds_type, rds_pattern)
        .await?
        .map(|target| target.endpoint))
}

/// The RDS endpoint together with the identifier of the instance or cluster
/// it belongs to (the first available match, as `get_rds_endpoint` picks).
pub async fn get_rds_target(
    clients: &AwsClients,
    rds_type: &str,
    rds_pattern: &str,
) -> Result<Option<RdsTarget>, AppError> {
    match rds_type {
        "cluster" => {
            let response = clients
                .rds
                .describe_db_clusters()
                .send()
                .await
                .map_err(|e| AppError::Aws(format!("Failed to describe DB clusters: {}", e)))?;

            Ok(first_matching_target(
                response.db_clusters().iter().map(|cluster| {
                    (
                        cluster.db_cluster_identifier().unwrap_or_default(),
                        cluster.status().unwrap_or_default(),
                        cluster.endpoint(),
                    )
                }),
                rds_pattern,
            ))
        }
        "instance" => {
            let response = clients
                .rds
                .describe_db_instances()
                .send()
                .await
                .map_err(|e| AppError::Aws(format!("Failed to describe DB instances: {}", e)))?;

            Ok(first_matching_target(
                response.db_instances().iter().map(|instance| {
                    (
                        instance.db_instance_identifier().unwrap_or_default(),
                        instance.db_instance_status().unwrap_or_default(),
                        instance.endpoint().and_then(|e| e.address()),
                    )
                }),
                rds_pattern,
            ))
        }
        _ => Ok(None),
    }
}

/// Get the RDS port (cluster or instance based on rdsType).
pub async fn get_rds_port(
    clients: &AwsClients,
    rds_type: &str,
    rds_pattern: &str,
    fallback_port: &str,
) -> Result<String, AppError> {
    match rds_type {
        "cluster" => {
            let response = clients
                .rds
                .describe_db_clusters()
                .send()
                .await
                .map_err(|e| AppError::Aws(format!("Failed to describe DB clusters: {}", e)))?;

            for cluster in response.db_clusters() {
                let status = cluster.status().unwrap_or_default();
                let identifier = cluster.db_cluster_identifier().unwrap_or_default();

                if status == "available"
                    && identifier.contains(rds_pattern)
                    && let Some(port) = cluster.port()
                {
                    return Ok(port.to_string());
                }
            }

            Ok(fallback_port.to_string())
        }
        "instance" => {
            let response = clients
                .rds
                .describe_db_instances()
                .send()
                .await
                .map_err(|e| AppError::Aws(format!("Failed to describe DB instances: {}", e)))?;

            for instance in response.db_instances() {
                let status = instance.db_instance_status().unwrap_or_default();
                let identifier = instance.db_instance_identifier().unwrap_or_default();

                if status == "available"
                    && identifier.contains(rds_pattern)
                    && let Some(endpoint) = instance.endpoint()
                    && let Some(port) = endpoint.port()
                {
                    return Ok(port.to_string());
                }
            }

            Ok(fallback_port.to_string())
        }
        _ => Ok(fallback_port.to_string()),
    }
}

/// Wait for SSM agent on an instance to become online.
pub async fn wait_for_ssm_agent_ready(
    clients: &AwsClients,
    instance_id: &str,
    max_retries: u32,
    retry_delay_ms: u64,
    stabilization_ms: u64,
) -> Result<bool, AppError> {
    for attempt in 1..=max_retries {
        let response = clients
            .ssm
            .describe_instance_information()
            .filters(
                aws_sdk_ssm::types::InstanceInformationStringFilter::builder()
                    .key("InstanceIds")
                    .values(instance_id)
                    .build()
                    .map_err(|e| AppError::Aws(format!("Failed to build filter: {}", e)))?,
            )
            .send()
            .await
            .map_err(|e| {
                AppError::Aws(format!("Failed to describe instance information: {}", e))
            })?;

        let instances = response.instance_information_list();
        if !instances.is_empty()
            && let Some(ping_status) = instances[0].ping_status()
            && ping_status == &aws_sdk_ssm::types::PingStatus::Online
        {
            tokio::time::sleep(tokio::time::Duration::from_millis(stabilization_ms)).await;
            return Ok(true);
        }

        if attempt < max_retries {
            tokio::time::sleep(tokio::time::Duration::from_millis(retry_delay_ms)).await;
        }
    }

    Ok(false)
}

/// Start an SSM port forwarding session to a remote host (through a bastion).
/// Uses AWS-StartPortForwardingSessionToRemoteHost document.
pub async fn start_remote_port_forwarding_session(
    clients: &AwsClients,
    bastion_instance_id: &str,
    remote_host: &str,
    remote_port: &str,
    local_port: &str,
) -> Result<aws_sdk_ssm::operation::start_session::StartSessionOutput, AppError> {
    let response = clients
        .ssm
        .start_session()
        .target(bastion_instance_id)
        .document_name("AWS-StartPortForwardingSessionToRemoteHost")
        .parameters("host", vec![remote_host.to_string()])
        .parameters("portNumber", vec![remote_port.to_string()])
        .parameters("localPortNumber", vec![local_port.to_string()])
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to start SSM session: {}", e)))?;

    Ok(response)
}

/// Backward-compatible alias for existing RDS tunnel code.
pub async fn start_session(
    clients: &AwsClients,
    instance_id: &str,
    rds_endpoint: &str,
    remote_port: &str,
    local_port: &str,
) -> Result<aws_sdk_ssm::operation::start_session::StartSessionOutput, AppError> {
    start_remote_port_forwarding_session(clients, instance_id, rds_endpoint, remote_port, local_port)
        .await
}

/// Start an SSM port forwarding session directly to an instance (no bastion).
/// Uses AWS-StartPortForwardingSession document — the target instance must have SSM agent.
pub async fn start_direct_port_forwarding_session(
    clients: &AwsClients,
    instance_id: &str,
    remote_port: &str,
    local_port: &str,
) -> Result<aws_sdk_ssm::operation::start_session::StartSessionOutput, AppError> {
    let response = clients
        .ssm
        .start_session()
        .target(instance_id)
        .document_name("AWS-StartPortForwardingSession")
        .parameters("portNumber", vec![remote_port.to_string()])
        .parameters("localPortNumber", vec![local_port.to_string()])
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to start direct SSM session: {}", e)))?;

    Ok(response)
}

/// Find a running EC2 instance by Name tag pattern. Returns (instance_id, private_ip).
pub async fn find_ec2_instance(
    clients: &AwsClients,
    name_pattern: &str,
) -> Result<(String, String), AppError> {
    let response = clients
        .ec2
        .describe_instances()
        .filters(
            aws_sdk_ec2::types::Filter::builder()
                .name("tag:Name")
                .values(name_pattern)
                .build(),
        )
        .filters(
            aws_sdk_ec2::types::Filter::builder()
                .name("instance-state-name")
                .values("running")
                .build(),
        )
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to describe instances: {}", e)))?;

    for reservation in response.reservations() {
        for instance in reservation.instances() {
            if let (Some(id), Some(ip)) =
                (instance.instance_id(), instance.private_ip_address())
            {
                return Ok((id.to_string(), ip.to_string()));
            }
        }
    }

    Err(AppError::Aws(format!(
        "No running EC2 instance found with tag Name={}.",
        name_pattern
    )))
}

/// Find the private IP of a running ECS task in the given cluster/service.
/// Requires awsvpc network mode (Fargate or EC2 with awsvpc).
pub async fn find_ecs_task_ip(
    clients: &AwsClients,
    cluster: &str,
    service: &str,
) -> Result<String, AppError> {
    // List running tasks for the service
    let list_response = clients
        .ecs
        .list_tasks()
        .cluster(cluster)
        .service_name(service)
        .desired_status(aws_sdk_ecs::types::DesiredStatus::Running)
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to list ECS tasks: {}", e)))?;

    let task_arns = list_response.task_arns();
    if task_arns.is_empty() {
        return Err(AppError::Aws(format!(
            "No running tasks found for service '{}' in cluster '{}'.",
            service, cluster
        )));
    }

    // Describe the first running task to get its ENI
    let describe_response = clients
        .ecs
        .describe_tasks()
        .cluster(cluster)
        .tasks(&task_arns[0])
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to describe ECS task: {}", e)))?;

    let tasks = describe_response.tasks();
    if tasks.is_empty() {
        return Err(AppError::Aws("ECS task description returned empty.".to_string()));
    }

    // Extract ENI from task attachments (awsvpc mode)
    for attachment in tasks[0].attachments() {
        if attachment.r#type() == Some("ElasticNetworkInterface") {
            for detail in attachment.details() {
                if detail.name() == Some("privateIPv4Address")
                    && let Some(ip) = detail.value()
                {
                    return Ok(ip.to_string());
                }
            }
        }
    }

    Err(AppError::Aws(format!(
        "Could not find private IP for ECS task in service '{}'. Ensure awsvpc network mode is used.",
        service
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Secrets keyed by name; prefix listing is in name order like the API.
    struct FakeStore(BTreeMap<String, String>);

    impl FakeStore {
        fn with(names: &[&str]) -> Self {
            Self(
                names
                    .iter()
                    .map(|n| {
                        (
                            n.to_string(),
                            format!(r#"{{"username":"user-of-{}","password":"pw-of-{}"}}"#, n, n),
                        )
                    })
                    .collect(),
            )
        }
    }

    impl SecretStore for FakeStore {
        fn names_with_prefix<'a>(&'a self, prefix: &'a str) -> BoxFuture<'a, Result<Vec<String>, AppError>> {
            Box::pin(async move { Ok(self.0.keys().filter(|k| k.starts_with(prefix)).cloned().collect()) })
        }

        fn secret_string<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<String>, AppError>> {
            Box::pin(async move { Ok(self.0.get(name).cloned()) })
        }
    }

    async fn select(store: &FakeStore, prefix: &str, fallback: Option<&str>) -> Result<DbCredentials, AppError> {
        fetch_credentials(store, prefix, "covered_db", None, None, None, fallback).await
    }

    #[test]
    fn ensure_account_mismatch_message_names_both_accounts() {
        let err = ensure_account("231030592327", "073813807522", "prod").unwrap_err();
        assert!(matches!(err, AppError::Aws(_)), "{err:?}");
        let msg = err.to_string();
        assert!(
            msg.contains(
                "Profile signed in to account 073813807522, but environment 'prod' is account 231030592327. Pick another profile in Settings."
            ),
            "{msg}"
        );
    }

    #[test]
    fn ensure_account_match_ok() {
        assert!(ensure_account("231030592327", "231030592327", "prod").is_ok());
    }

    #[test]
    fn fallback_name_follows_the_iac_convention() {
        assert_eq!(fallback_secret_name("covered-db-staging"), "rds/covered-db-staging-credentials");
        assert_eq!(fallback_secret_name("covered-db"), "rds/covered-db-credentials");
    }

    #[tokio::test]
    async fn prefix_hit_wins_over_the_fallback() {
        let store = FakeStore::with(&["rds!db-1234", "rds/covered-db-staging-credentials"]);
        let creds = select(&store, "rds!db", Some("rds/covered-db-staging-credentials")).await.unwrap();
        assert_eq!(creds.secret_name, "rds!db-1234");
        assert_eq!(creds.username, "user-of-rds!db-1234");
        assert!(!creds.via_fallback);
    }

    #[tokio::test]
    async fn prefix_miss_uses_the_exact_fallback_name() {
        let store = FakeStore::with(&[
            "rds/covered-db-staging-credentials",
            "rds/covered-db-staging-credentials-old",
            "rds/covered-db-staging-local-credentials",
            "rds/covered-db-staging-master-credentials",
        ]);
        let creds = select(&store, "rds!db", Some("rds/covered-db-staging-credentials")).await.unwrap();
        assert_eq!(creds.secret_name, "rds/covered-db-staging-credentials");
        assert_eq!(creds.password, "pw-of-rds/covered-db-staging-credentials");
        assert!(creds.via_fallback);
    }

    #[tokio::test]
    async fn both_misses_name_both_lookups() {
        // Only the variants that must never be used exist.
        let store = FakeStore::with(&[
            "rds/covered-db-staging-master-credentials",
            "rds/covered-db-staging-local-credentials",
        ]);
        let err = select(&store, "rds!db", Some("rds/covered-db-staging-credentials"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("prefix 'rds!db'"), "{}", err);
        assert!(err.contains("'rds/covered-db-staging-credentials'"), "{}", err);
    }

    #[tokio::test]
    async fn without_a_fallback_the_old_error_stays() {
        let store = FakeStore::with(&[]);
        let err = select(&store, "rds!db", None).await.unwrap_err().to_string();
        assert!(err.contains("No secret found matching prefix 'rds!db'."), "{}", err);
    }

    #[tokio::test]
    async fn secret_path_bypasses_prefix_and_fallback() {
        let store = FakeStore::with(&["custom/path", "rds!db-1"]);
        let creds = fetch_credentials(&store, "rds!db", "db", Some("custom/path"), None, None, Some("rds/x-credentials"))
            .await
            .unwrap();
        assert_eq!(creds.secret_name, "custom/path");
        assert!(!creds.via_fallback);
    }

    #[test]
    fn target_is_the_first_available_match_in_api_order() {
        // Prod lists the primary before its read replica; both match "covered-db".
        let instances = [
            ("covered-db", "available", Some("covered-db.abc.us-west-1.rds.amazonaws.com")),
            ("covered-db-ro", "available", Some("covered-db-ro.abc.us-west-1.rds.amazonaws.com")),
        ];
        let target = first_matching_target(instances, "covered-db").unwrap();
        assert_eq!(target.identifier, "covered-db");
        assert_eq!(target.endpoint, "covered-db.abc.us-west-1.rds.amazonaws.com");
        assert_eq!(fallback_secret_name(&target.identifier), "rds/covered-db-credentials");

        // The identifier always belongs to the endpoint that is used.
        let stopped_primary = [
            ("covered-db", "stopped", Some("covered-db.abc.us-west-1.rds.amazonaws.com")),
            ("covered-db-ro", "available", Some("covered-db-ro.abc.us-west-1.rds.amazonaws.com")),
        ];
        let target = first_matching_target(stopped_primary, "covered-db").unwrap();
        assert_eq!(target.identifier, "covered-db-ro");
        assert!(target.endpoint.starts_with("covered-db-ro."));

        assert_eq!(first_matching_target([("other", "available", Some("x"))], "covered-db"), None);
    }

    #[test]
    fn exact_identifier_wins_even_when_listed_after_a_substring_match() {
        let instances = [
            ("covered-db-ro", "available", Some("covered-db-ro.abc.us-west-1.rds.amazonaws.com")),
            ("covered-db", "available", Some("covered-db.abc.us-west-1.rds.amazonaws.com")),
        ];
        let target = first_matching_target(instances, "covered-db").unwrap();
        assert_eq!(target.identifier, "covered-db");
        assert_eq!(target.endpoint, "covered-db.abc.us-west-1.rds.amazonaws.com");
    }

    #[test]
    fn without_an_exact_identifier_the_first_substring_match_is_used() {
        let instances = [
            ("covered-db-dev", "available", Some("covered-db-dev.abc.us-west-1.rds.amazonaws.com")),
            ("covered-db-dev-ro", "available", Some("covered-db-dev-ro.abc.us-west-1.rds.amazonaws.com")),
        ];
        let target = first_matching_target(instances, "covered-db").unwrap();
        assert_eq!(target.identifier, "covered-db-dev");
    }

    #[test]
    fn an_unavailable_exact_identifier_falls_back_to_a_substring_match() {
        let instances = [
            ("covered-db-ro", "available", Some("covered-db-ro.abc.us-west-1.rds.amazonaws.com")),
            ("covered-db", "modifying", Some("covered-db.abc.us-west-1.rds.amazonaws.com")),
        ];
        let target = first_matching_target(instances, "covered-db").unwrap();
        assert_eq!(target.identifier, "covered-db-ro");
        assert!(target.endpoint.starts_with("covered-db-ro."));
    }
}
