use std::sync::Arc;

use aionui_api_types::{OpenClawBuildExtra, OpenClawGatewayConfig, RemoteBuildExtra};
use aionui_common::AppError;
use tracing::warn;

use crate::agent_task::AgentInstance;
use crate::factory::AgentFactoryDeps;
use crate::factory::context::FactoryContext;
use crate::types::BuildTaskOptions;

pub(super) async fn build(
    deps: Arc<AgentFactoryDeps>,
    options: BuildTaskOptions,
    ctx: FactoryContext,
) -> Result<AgentInstance, AppError> {
    let extra: RemoteBuildExtra = serde_json::from_value(options.extra.clone())
        .map_err(|e| AppError::BadRequest(format!("Invalid Remote build options: {e}")))?;
    let row = deps
        .remote_agent_repo
        .find_by_id(&extra.remote_agent_id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to load remote agent config: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("Remote agent '{}' not found", extra.remote_agent_id)))?;
    let auth_token = row
        .auth_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .and_then(|encrypted| {
            aionui_common::decrypt_string(encrypted, &deps.encryption_key)
                .map_err(|e| {
                    warn!(error = %e, "Failed to decrypt remote agent auth_token");
                })
                .ok()
        });

    // Delegate to OpenClaw gateway — RemoteAgentManager's WebSocket is broken.
    // Build gateway config from the remote agent's URL and auth token.
    let (host, port) = parse_ws_url(&row.url).unwrap_or_else(|| {
        warn!(url = %row.url, "Failed to parse remote agent URL, using as-is");
        (row.url.clone(), None)
    });

    let openclaw_extra = OpenClawBuildExtra {
        backend: Some("remote".to_owned()),
        agent_name: Some(row.name.clone()),
        gateway: OpenClawGatewayConfig {
            host: Some(host),
            port,
            token: auth_token.clone(),
            password: None,
            use_external_gateway: true,
            cli_path: Some(row.url.clone()),
        },
        skills: Vec::new(),
        preset_assistant_id: None,
        cron_job_id: None,
        session_key: None,
    };

    let mut openclaw_options = options;
    openclaw_options.extra = serde_json::to_value(&openclaw_extra)
        .map_err(|e| AppError::Internal(format!("Failed to serialize OpenClaw build extra: {e}")))?;

    crate::factory::openclaw::build(deps, openclaw_options, ctx).await
}

/// Parse a WebSocket URL like `ws://127.0.0.1:18790` into (host, port).
fn parse_ws_url(url: &str) -> Option<(String, Option<u16>)> {
    // Strip ws:// or wss:// prefix
    let without_scheme = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))?;

    // Split host:port
    if let Some(colon_pos) = without_scheme.rfind(':') {
        let host = without_scheme[..colon_pos].to_owned();
        let port = without_scheme[colon_pos + 1..].parse::<u16>().ok();
        Some((host, port))
    } else {
        Some((without_scheme.to_owned(), None))
    }
}
