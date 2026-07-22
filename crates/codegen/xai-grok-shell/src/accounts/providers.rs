//! Provider adapters for validation and authenticated model discovery.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::storage::{AccountEntry, DiscoveredModel, ProviderId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryProtocol {
    OpenAiCompatible,
    Anthropic,
}

fn protocol(account: &AccountEntry) -> DiscoveryProtocol {
    if account.provider.as_str() == ProviderId::ANTHROPIC {
        DiscoveryProtocol::Anthropic
    } else {
        DiscoveryProtocol::OpenAiCompatible
    }
}

fn models_url(account: &AccountEntry) -> Result<String> {
    let base = account
        .base_url
        .as_deref()
        .or_else(|| account.provider.default_base_url())
        .context("provider requires a base URL")?
        .trim_end_matches('/');
    Ok(format!("{base}/models"))
}

fn redact(mut message: String, account: &AccountEntry) -> String {
    if let Some(secret) = account
        .auth
        .credential()
        .filter(|secret| !secret.is_empty())
    {
        message = message.replace(secret, "[REDACTED]");
    }
    message
}

/// Validate credentials and return the provider's current model catalog.
/// The last successful result is persisted by the caller for offline use.
pub async fn discover_models(account: &AccountEntry) -> Result<Vec<DiscoveredModel>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let mut request = client.get(models_url(account)?);
    match protocol(account) {
        DiscoveryProtocol::Anthropic => {
            if let Some(secret) = account.auth.credential() {
                request = request.header("x-api-key", secret);
            }
            request = request.header("anthropic-version", "2023-06-01");
        }
        DiscoveryProtocol::OpenAiCompatible => {
            if let Some(secret) = account.auth.credential() {
                request = request.bearer_auth(secret);
            }
        }
    }

    let response = request
        .send()
        .await
        .map_err(|error| anyhow::anyhow!(redact(error.to_string(), account)))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| anyhow::anyhow!(redact(error.to_string(), account)))?;
    if !status.is_success() {
        let summary = redact(crate::util::truncate(&body, 300).to_owned(), account);
        bail!("provider validation failed with HTTP {status}: {summary}");
    }

    #[derive(Deserialize)]
    struct Catalog {
        #[serde(default)]
        data: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        id: String,
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        context_window: Option<u64>,
    }
    let catalog: Catalog = serde_json::from_str(&body)
        .map_err(|error| anyhow::anyhow!("invalid provider model catalog: {error}"))?;
    let mut models = catalog
        .data
        .into_iter()
        .filter(|model| !model.id.trim().is_empty())
        .map(|model| DiscoveredModel {
            name: model
                .display_name
                .or(model.name)
                .unwrap_or_else(|| model.id.clone()),
            id: model.id,
            context_window: model.context_window,
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::storage::AccountAuth;

    #[test]
    fn errors_redact_account_secret() {
        let account = AccountEntry {
            id: "a".into(),
            name: "A".into(),
            provider: ProviderId::parse("openai"),
            auth: AccountAuth::ApiKey {
                secret: "sk-private".into(),
            },
            enabled: true,
            base_url: None,
            model_allowlist: vec![],
            discovered_models: vec![],
            models_refreshed_at: None,
            created_at: None,
            cost_tier: None,
        };
        assert_eq!(redact("bad sk-private".into(), &account), "bad [REDACTED]");
    }

    #[tokio::test]
    async fn authenticated_discovery_uses_mock_server_and_parses_models() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 4096];
            let read = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.contains("GET /v1/models"));
            assert!(request.contains("authorization: Bearer sk-test"));
            let body = r#"{"data":[{"id":"model-b"},{"id":"model-a","context_window":128000}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let account = AccountEntry {
            id: "a".into(),
            name: "A".into(),
            provider: ProviderId::parse("custom"),
            auth: AccountAuth::ApiKey {
                secret: "sk-test".into(),
            },
            enabled: true,
            base_url: Some(format!("http://{address}/v1")),
            model_allowlist: vec![],
            discovered_models: vec![],
            models_refreshed_at: None,
            created_at: None,
            cost_tier: None,
        };
        let models = discover_models(&account).await.unwrap();
        server.await.unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["model-a", "model-b"]
        );
        assert_eq!(models[0].context_window, Some(128_000));
    }
}
