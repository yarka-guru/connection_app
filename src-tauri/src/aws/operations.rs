use crate::aws::credentials::AwsClients;
use crate::error::AppError;

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

/// Get database credentials from Secrets Manager.
pub async fn get_connection_credentials(
    clients: &AwsClients,
    secret_prefix: &str,
    database: &str,
) -> Result<DbCredentials, AppError> {
    let list_response = clients
        .secrets_manager
        .list_secrets()
        .filters(
            aws_sdk_secretsmanager::types::Filter::builder()
                .key(aws_sdk_secretsmanager::types::FilterNameStringType::Name)
                .values(secret_prefix)
                .build(),
        )
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to list secrets: {:?}", e)))?;

    let secrets = list_response.secret_list();
    if secrets.is_empty() {
        return Err(AppError::Aws(format!(
            "No secret found matching prefix '{}'.",
            secret_prefix
        )));
    }

    let secret_name = secrets[0]
        .name()
        .ok_or_else(|| AppError::Aws("Secret has no name".to_string()))?
        .to_string();

    let get_response = clients
        .secrets_manager
        .get_secret_value()
        .secret_id(&secret_name)
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to get secret value: {:?}", e)))?;

    let secret_string = get_response
        .secret_string()
        .ok_or_else(|| {
            AppError::Aws(format!(
                "Secret '{}' has no SecretString value.",
                secret_name
            ))
        })?;

    let credentials: serde_json::Value = serde_json::from_str(secret_string).map_err(|e| {
        AppError::Aws(format!(
            "Failed to parse credentials from secret '{}': {}",
            secret_name, e
        ))
    })?;

    let username = credentials
        .get("username")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AppError::Aws(format!(
                "Secret '{}' is missing required field: username",
                secret_name
            ))
        })?
        .to_string();

    let password = credentials
        .get("password")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AppError::Aws(format!(
                "Secret '{}' is missing required field: password",
                secret_name
            ))
        })?
        .to_string();

    Ok(DbCredentials {
        username,
        password,
        database: database.to_string(),
        secret_name,
    })
}

/// Find a running bastion instance tagged with the given Name pattern.
pub async fn find_bastion_instance(
    clients: &AwsClients,
    bastion_pattern: &str,
) -> Result<String, AppError> {
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
        .map_err(|e| AppError::Aws(format!("Failed to describe instances: {:?}", e)))?;

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
    match rds_type {
        "cluster" => {
            let response = clients
                .rds
                .describe_db_clusters()
                .send()
                .await
                .map_err(|e| AppError::Aws(format!("Failed to describe DB clusters: {:?}", e)))?;

            for cluster in response.db_clusters() {
                let status = cluster.status().unwrap_or_default();
                let identifier = cluster.db_cluster_identifier().unwrap_or_default();

                if status == "available" && identifier.ends_with(rds_pattern) {
                    return Ok(cluster.endpoint().map(|e| e.to_string()));
                }
            }

            Ok(None)
        }
        "instance" => {
            let response = clients
                .rds
                .describe_db_instances()
                .send()
                .await
                .map_err(|e| AppError::Aws(format!("Failed to describe DB instances: {:?}", e)))?;

            for instance in response.db_instances() {
                let status = instance.db_instance_status().unwrap_or_default();
                let identifier = instance.db_instance_identifier().unwrap_or_default();

                if status == "available" && identifier.contains(rds_pattern) {
                    return Ok(instance
                        .endpoint()
                        .and_then(|e| e.address())
                        .map(|a| a.to_string()));
                }
            }

            Ok(None)
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
                .map_err(|e| AppError::Aws(format!("Failed to describe DB clusters: {:?}", e)))?;

            for cluster in response.db_clusters() {
                let status = cluster.status().unwrap_or_default();
                let identifier = cluster.db_cluster_identifier().unwrap_or_default();

                if status == "available"
                    && identifier.ends_with(rds_pattern)
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
                .map_err(|e| AppError::Aws(format!("Failed to describe DB instances: {:?}", e)))?;

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

/// Terminate a bastion instance.
pub async fn terminate_bastion_instance(
    clients: &AwsClients,
    instance_id: &str,
) -> Result<(), AppError> {
    clients
        .ec2
        .terminate_instances()
        .instance_ids(instance_id)
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to terminate bastion: {:?}", e)))?;

    Ok(())
}

/// Wait for a new bastion instance to appear (after terminating old one).
pub async fn wait_for_new_bastion_instance(
    clients: &AwsClients,
    old_instance_id: &str,
    bastion_pattern: &str,
    max_retries: u32,
    retry_delay_ms: u64,
) -> Result<Option<String>, AppError> {
    for attempt in 1..=max_retries {
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
            .map_err(|e| AppError::Aws(format!("Failed to describe instances: {:?}", e)))?;

        let mut new_instance_id = None;
        for reservation in response.reservations() {
            for instance in reservation.instances() {
                if let Some(id) = instance.instance_id()
                    && id != old_instance_id
                    && id != "None"
                {
                    new_instance_id = Some(id.to_string());
                    break;
                }
            }
            if new_instance_id.is_some() {
                break;
            }
        }

        if let Some(ref new_id) = new_instance_id {
            let is_ready =
                wait_for_ssm_agent_ready(clients, new_id, 10, 3000, 10000).await?;
            if is_ready {
                return Ok(new_instance_id);
            }
        }

        if attempt < max_retries {
            tokio::time::sleep(tokio::time::Duration::from_millis(retry_delay_ms)).await;
        }
    }

    Ok(None)
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
                AppError::Aws(format!("Failed to describe instance information: {:?}", e))
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

/// Start an SSM port forwarding session. Returns the session response needed by the plugin.
pub async fn start_session(
    clients: &AwsClients,
    instance_id: &str,
    rds_endpoint: &str,
    remote_port: &str,
    local_port: &str,
) -> Result<aws_sdk_ssm::operation::start_session::StartSessionOutput, AppError> {
    let response = clients
        .ssm
        .start_session()
        .target(instance_id)
        .document_name("AWS-StartPortForwardingSessionToRemoteHost")
        .parameters("host", vec![rds_endpoint.to_string()])
        .parameters("portNumber", vec![remote_port.to_string()])
        .parameters("localPortNumber", vec![local_port.to_string()])
        .send()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to start SSM session: {:?}", e)))?;

    Ok(response)
}
