//! Client-side check of the StreamUrl returned by `ssm:StartSession`.
//!
//! The data channel connects to whatever StreamUrl the API returns and sends
//! it the session token. If the SSM endpoint is overridden (`endpoint_url` in
//! `~/.aws/config`, `AWS_ENDPOINT_URL_SSM`, a proxy) the response could point
//! anywhere. This rejects hosts that are not an AWS stream endpoint before the
//! WebSocket is opened. The server still verifies the token; this is
//! defense-in-depth, ported from session-manager-plugin 1.2.889.0.

use aws_sdk_ssm::config::endpoint::{DefaultResolver, Params, ResolveEndpoint};
use regex::Regex;
use std::sync::LazyLock;
use tokio_tungstenite::tungstenite::http::Uri;

/// Comma-separated extra StreamUrl hosts to trust by exact match. Same name
/// as the official plugin, so one setting covers both tools.
pub const ADDITIONAL_STREAM_HOSTS_ENV: &str = "AWS_SSM_PLUGIN_ADDITIONAL_STREAM_HOSTS";

/// The service label must contain one of these.
const SERVICE_KEYWORDS: &[&str] = &["ssmmessages", "fips", "mgs", "gamma", "beta"];

/// Trusted for StreamUrl hosts in every region.
const AMAZON_DNS_SUFFIX: &str = "amazon.com";

/// A region name such as "us-east-1", or a three-letter code such as "iad".
static REGION_LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-z]{2,4}(-[a-z]+)+-[0-9]+|[a-z]{3})$").unwrap());

/// Validate a StreamUrl before connecting to it.
pub async fn validate_stream_url(stream_url: &str) -> Result<(), String> {
    let extra_hosts = std::env::var(ADDITIONAL_STREAM_HOSTS_ENV).unwrap_or_default();
    validate_stream_url_with(stream_url, &extra_hosts).await
}

async fn validate_stream_url_with(stream_url: &str, extra_hosts: &str) -> Result<(), String> {
    if stream_url.trim().is_empty() {
        return Err("StreamUrl is empty".to_string());
    }
    let uri: Uri = stream_url
        .parse()
        .map_err(|e| format!("StreamUrl is not a valid URL: {}", e))?;
    // AWS only issues wss:// stream URLs; plain ws:// would send the token in clear text.
    match uri.scheme_str() {
        Some(s) if s.eq_ignore_ascii_case("wss") => {}
        other => {
            return Err(format!(
                "StreamUrl has unexpected scheme {:?}, expected \"wss\"",
                other.unwrap_or("")
            ))
        }
    }
    let authority = uri.authority().ok_or("StreamUrl has no host")?;
    if authority.as_str().contains('@') {
        return Err("StreamUrl must not contain userinfo".to_string());
    }
    let host = authority.host().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return Err("StreamUrl has no host".to_string());
    }

    if is_additional_trusted_host(&host, extra_hosts) {
        return Ok(());
    }

    let not_aws = || format!("StreamUrl host {:?} is not a recognized AWS stream endpoint", host);
    let (service, region, suffix) = split_stream_host(&host).ok_or_else(not_aws)?;
    if !REGION_LABEL.is_match(region) || !SERVICE_KEYWORDS.iter().any(|k| service.contains(k)) {
        return Err(not_aws());
    }
    if trusted_dns_suffixes(region).await.iter().any(|s| s == suffix) {
        Ok(())
    } else {
        Err(not_aws())
    }
}

fn is_additional_trusted_host(host: &str, extra_hosts: &str) -> bool {
    extra_hosts
        .split(',')
        .map(|e| e.trim().trim_end_matches('.').to_ascii_lowercase())
        .any(|e| !e.is_empty() && e == host)
}

/// Split a host of the form `<service>.<region>.<suffix>` or
/// `<vpce-id>.<service>.<region>.vpce.<suffix>` into its parts.
fn split_stream_host(host: &str) -> Option<(&str, &str, &str)> {
    let labels: Vec<&str> = host.split('.').collect();
    let (service, region, skip) =
        if labels.len() >= 6 && labels[0].starts_with("vpce-") && labels[3] == "vpce" {
            (labels[1], labels[2], 4)
        } else if labels.len() >= 4 {
            (labels[0], labels[1], 2)
        } else {
            return None;
        };
    let offset: usize = labels[..skip].iter().map(|l| l.len() + 1).sum();
    Some((service, region, &host[offset..]))
}

/// DNS suffixes a StreamUrl host may end with: the SDK's standard and
/// dual-stack SSM endpoint suffixes for the region (so new partitions need no
/// code change), plus amazon.com.
async fn trusted_dns_suffixes(region: &str) -> Vec<String> {
    let resolver = DefaultResolver::new();
    let mut suffixes = vec![AMAZON_DNS_SUFFIX.to_string()];
    for dual_stack in [false, true] {
        let Ok(params) = Params::builder()
            .region(region)
            .use_dual_stack(dual_stack)
            .use_fips(false)
            .build()
        else {
            continue;
        };
        let Ok(endpoint) = resolver.resolve_endpoint(&params).await else {
            continue;
        };
        let Ok(uri) = endpoint.url().parse::<Uri>() else {
            continue;
        };
        if let Some(host) = uri.host() {
            let host = host.to_ascii_lowercase();
            let suffix = host.strip_prefix("ssm.").unwrap_or(&host);
            let suffix = suffix.strip_prefix(&format!("{}.", region)).unwrap_or(suffix);
            if !suffix.is_empty() {
                suffixes.push(suffix.to_string());
            }
        }
    }
    suffixes
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn check(url: &str) -> Result<(), String> {
        validate_stream_url_with(url, "").await
    }

    #[tokio::test]
    async fn accepts_real_aws_stream_urls() {
        for url in [
            "wss://ssmmessages.us-east-1.amazonaws.com/v1/data-channel/user-0abc?role=publish_subscribe&cell-number=AAAA",
            "wss://ssmmessages.eu-central-1.amazonaws.com/v1/data-channel/s",
            "wss://ssmmessages-fips.us-gov-west-1.amazonaws.com/v1/data-channel/s",
            "wss://ssmmessages.cn-north-1.amazonaws.com.cn/v1/data-channel/s",
            "wss://ssmmessages.us-east-1.api.aws/v1/data-channel/s",
            "wss://vpce-0123456789abcdef0-abcdefgh.ssmmessages.us-east-1.vpce.amazonaws.com/v1/data-channel/s",
            "wss://ssmmessages.us-east-1.amazon.com/v1/data-channel/s",
            "WSS://SSMMESSAGES.US-EAST-1.AMAZONAWS.COM./v1/data-channel/s",
        ] {
            assert_eq!(check(url).await, Ok(()), "{}", url);
        }
    }

    #[tokio::test]
    async fn rejects_non_aws_hosts() {
        for url in [
            "wss://ssmmessages.us-east-1.evil.com/v1/data-channel/s",
            "wss://ssmmessages.us-east-1.amazonaws.com.evil.com/v1/data-channel/s",
            "wss://evil.com/v1/data-channel/s",
            "wss://amazonaws.com/v1/data-channel/s",
            "wss://evil.us-east-1.amazonaws.com/v1/data-channel/s",
            "wss://ssmmessages.not_a_region.amazonaws.com/v1/data-channel/s",
            "wss://ssmmessages.cn-north-1.evilamazonaws.com.cn/v1/data-channel/s",
            "wss://127.0.0.1:8443/v1/data-channel/s",
        ] {
            assert!(check(url).await.is_err(), "{}", url);
        }
    }

    #[tokio::test]
    async fn rejects_bad_scheme_userinfo_and_garbage() {
        for url in [
            "",
            "   ",
            "ws://ssmmessages.us-east-1.amazonaws.com/v1/data-channel/s",
            "https://ssmmessages.us-east-1.amazonaws.com/v1/data-channel/s",
            "wss://user:pw@ssmmessages.us-east-1.amazonaws.com/v1/data-channel/s",
            "/v1/data-channel/s",
            "not a url",
        ] {
            assert!(check(url).await.is_err(), "{:?}", url);
        }
    }

    #[tokio::test]
    async fn additional_hosts_are_exact_match() {
        let extra = " localhost. , stream.example.internal ";
        assert_eq!(validate_stream_url_with("wss://localhost:4566/s", extra).await, Ok(()));
        assert_eq!(
            validate_stream_url_with("wss://Stream.Example.Internal/s", extra).await,
            Ok(())
        );
        assert!(validate_stream_url_with("wss://x.stream.example.internal/s", extra)
            .await
            .is_err());
        // Scheme and userinfo checks still apply to extra hosts.
        assert!(validate_stream_url_with("ws://localhost/s", extra).await.is_err());
    }

    #[test]
    fn split_stream_host_shapes() {
        assert_eq!(
            split_stream_host("ssmmessages.us-east-1.amazonaws.com"),
            Some(("ssmmessages", "us-east-1", "amazonaws.com"))
        );
        assert_eq!(
            split_stream_host("vpce-1-a.ssmmessages.eu-west-1.vpce.amazonaws.com"),
            Some(("ssmmessages", "eu-west-1", "amazonaws.com"))
        );
        assert_eq!(split_stream_host("ssmmessages.us-east-1.com"), None);
    }
}
