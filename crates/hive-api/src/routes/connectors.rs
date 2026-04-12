use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::{shared_api_client, AppState, OAuthPendingMeta};

/// List all configured connectors.
pub(crate) async fn api_list_channels(State(state): State<AppState>) -> impl IntoResponse {
    // Prefer the runtime ConnectorService — it holds the authoritative configs
    // and secrets are automatically omitted via #[serde(skip_serializing)].
    if let Some(svc) = &state.connectors {
        let configs = svc.list_connector_configs();
        return Json(serde_json::to_value(&configs).unwrap_or(json!([]))).into_response();
    }
    // Fallback: read YAML directly (secrets are NOT included — they're in the OS keyring).
    let connectors_path = state.hivemind_home.join("connectors.yaml");
    let configs: Vec<hive_connectors::ConnectorConfig> = std::fs::read_to_string(&connectors_path)
        .ok()
        .and_then(|yaml| serde_yaml::from_str(&yaml).ok())
        .unwrap_or_default();
    Json(serde_json::to_value(&configs).unwrap_or(json!([]))).into_response()
}

/// Save/update connector configurations.
pub(crate) async fn api_save_channels(
    State(state): State<AppState>,
    Json(configs): Json<Vec<hive_connectors::ConnectorConfig>>,
) -> impl IntoResponse {
    let connector_svc = match &state.connectors {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "connector service not initialized" })),
            )
                .into_response()
        }
    };

    // Persist secrets to OS keyring before writing to YAML.
    for cfg in &configs {
        cfg.persist_secrets();
    }

    let connectors_path = state.hivemind_home.join("connectors.yaml");
    let yaml = match serde_yaml::to_string(&configs) {
        Ok(y) => y,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid config: {e}") })),
            )
                .into_response()
        }
    };
    if let Err(e) = std::fs::write(&connectors_path, &yaml) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to write connectors config: {e}") })),
        )
            .into_response();
    }

    // Reload connectors in the service with full configs (including secrets in memory)
    if let Err(e) = connector_svc.load_connectors(configs) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to load connectors: {e}") })),
        )
            .into_response();
    }

    // Restart background polling with new connector configs
    connector_svc.start_background_poll();

    // Notify listeners that connectors have changed
    if let Err(e) = state.event_bus.publish(
        "config.channels_reloaded",
        "hive-api",
        json!({ "connectors_path": connectors_path.display().to_string() }),
    ) {
        tracing::warn!(error = %e, "failed to publish config.channels_reloaded event");
    }

    Json(json!({ "ok": true })).into_response()
}

/// Test connector connectivity.
pub(crate) async fn api_test_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    let connector_svc = match &state.connectors {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "connector service not initialized" })),
            )
                .into_response()
        }
    };

    if connector_svc.registry().get(&channel_id).is_none() {
        if let Some(Json(body)) = &body {
            match serde_json::from_value::<hive_connectors::ConnectorConfig>(body.clone()) {
                Ok(mut cfg) => {
                    cfg.id = channel_id.clone();
                    cfg.restore_secrets();
                    if let Err(e) = connector_svc.load_single_temp(&cfg) {
                        return Json(json!({
                            "channel_id": channel_id,
                            "status": "error",
                            "error": format!("failed to create temp connector: {e:#}")
                        }))
                        .into_response();
                    }
                }
                Err(e) => {
                    return Json(json!({
                        "channel_id": channel_id,
                        "status": "error",
                        "error": format!("invalid config: {e}")
                    }))
                    .into_response();
                }
            }
        }
    }

    match connector_svc.test_connector(&channel_id).await {
        Ok(()) => Json(json!({
            "channel_id": channel_id,
            "status": "ok"
        }))
        .into_response(),
        Err(e) => {
            tracing::warn!(%channel_id, error = format!("{e:#}"), "connector test failed");
            Json(json!({
                "channel_id": channel_id,
                "status": "error",
                "error": format!("{e:#}")
            }))
            .into_response()
        }
    }
}

/// Discover available channels/guilds for a Discord or Slack connector.
pub(crate) async fn api_channel_discover(
    State(_state): State<AppState>,
    Path(_channel_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let connector_type = body.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match connector_type.to_lowercase().as_str() {
        "discord" => {
            let bot_token = match body.get("bot_token").and_then(|v| v.as_str()) {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => return Json(json!({ "error": "bot_token is required" })).into_response(),
            };
            let client = reqwest::Client::new();
            let guilds = match hive_connectors::providers::discord::api::list_guilds(&client, &bot_token).await {
                Ok(g) => g,
                Err(e) => return Json(json!({ "error": format!("Failed to list guilds: {e:#}") })).into_response(),
            };
            let mut all_channels = Vec::new();
            for guild in &guilds {
                if let Ok(channels) = hive_connectors::providers::discord::api::list_guild_channels(&client, &bot_token, &guild.id).await {
                    for ch in channels {
                        all_channels.push(json!({
                            "id": ch.id,
                            "name": ch.name,
                            "type": ch.type_,
                            "guild_id": guild.id,
                            "guild_name": guild.name,
                        }));
                    }
                }
            }
            Json(json!({
                "guilds": guilds.iter().map(|g| json!({ "id": g.id, "name": g.name })).collect::<Vec<_>>(),
                "channels": all_channels,
            })).into_response()
        }
        "slack" => {
            let bot_token = match body.get("bot_token").and_then(|v| v.as_str()) {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => return Json(json!({ "error": "bot_token is required" })).into_response(),
            };
            let client = reqwest::Client::new();
            let auth = match hive_connectors::providers::slack::api::auth_test(&client, &bot_token).await {
                Ok(a) => a,
                Err(e) => return Json(json!({ "error": format!("Auth test failed: {e:#}") })).into_response(),
            };
            let channels = match hive_connectors::providers::slack::api::conversations_list(&client, &bot_token).await {
                Ok(c) => c,
                Err(e) => return Json(json!({ "error": format!("Failed to list channels: {e:#}") })).into_response(),
            };
            Json(json!({
                "workspace_name": auth.team,
                "channels": channels.iter().map(|c| json!({
                    "id": c.id,
                    "name": c.name,
                    "is_channel": c.is_channel,
                    "is_im": c.is_im,
                    "is_member": c.is_member,
                })).collect::<Vec<_>>(),
            })).into_response()
        }
        _ => {
            Json(json!({ "error": format!("Channel type '{connector_type}' does not support discovery") })).into_response()
        }
    }
}

/// List available channels for a running connector's communication service.
pub(crate) async fn api_connector_list_channels(
    State(state): State<AppState>,
    Path(connector_id): Path<String>,
) -> impl IntoResponse {
    let svc = match &state.connectors {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "connector service not initialized" })),
            )
                .into_response()
        }
    };
    match svc.list_channels(&connector_id).await {
        Ok(channels) => Json(json!(channels)).into_response(),
        Err(e) => {
            tracing::warn!(%connector_id, error = format!("{e:#}"), "list_channels failed");
            (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("{e:#}") }))).into_response()
        }
    }
}

/// Start an OAuth device code flow for an email channel.
pub(crate) async fn api_channel_oauth_start(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    body: Option<Json<Value>>,
) -> impl IntoResponse {
    use hive_connectors::config::{
        AuthConfig, CalendarConfig, CommunicationConfig, ConnectorConfig, ContactsConfig,
        DriveConfig, ServicesConfig,
    };
    use hive_contracts::connectors::ConnectorProvider;

    let body = body.map(|b| b.0).unwrap_or(json!({}));

    let provider = if let Some(p) = body["provider"].as_str() {
        match p {
            "gmail" => ConnectorProvider::Gmail,
            "outlook" | "microsoft" => ConnectorProvider::Microsoft,
            "coinbase" => ConnectorProvider::Coinbase,
            other => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("unsupported provider: {other}. Use 'gmail', 'outlook', 'microsoft', or 'coinbase'.") })),
                )
                    .into_response()
            }
        }
    } else {
        let connectors_path = state.hivemind_home.join("connectors.yaml");
        let configs: Vec<ConnectorConfig> = std::fs::read_to_string(&connectors_path)
            .ok()
            .and_then(|yaml| serde_yaml::from_str(&yaml).ok())
            .unwrap_or_default();
        match configs.iter().find(|c| c.id == channel_id) {
            Some(cfg) => cfg.provider,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "specify 'provider' in request body (gmail, outlook, or microsoft)" })),
                )
                    .into_response()
            }
        }
    };

    // Save user-provided OAuth credentials to the keyring (e.g. for Coinbase which
    // has no built-in defaults).
    if let Some(cid) = body["client_id"].as_str().filter(|s| !s.is_empty()) {
        hive_connectors::secrets::save(&channel_id, "client_id_override", cid);
    }
    if let Some(cs) = body["client_secret"].as_str().filter(|s| !s.is_empty()) {
        hive_connectors::secrets::save(&channel_id, "custom_client_secret", cs);
    }

    let client_id = match crate::provider_auth::resolve_client_id(provider, &channel_id) {
        Some(id) => id,
        None => {
            let env_var = match provider {
                ConnectorProvider::Gmail => "HIVEMIND_GOOGLE_CLIENT_ID",
                ConnectorProvider::Microsoft => "HIVEMIND_OUTLOOK_CLIENT_ID",
                ConnectorProvider::Coinbase => "COINBASE_CLIENT_ID",
                _ => "N/A",
            };
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "missing_credentials",
                    "message": format!(
                        "No OAuth client ID configured. Set the {env_var} environment variable, \
                         or enter credentials in the channel's Custom provider settings."
                    ),
                    "env_var": env_var,
                    "setup_hint": match provider {
                        ConnectorProvider::Gmail =>
                            "Create a Google Cloud project → APIs & Services → Credentials → \
                             OAuth 2.0 Client ID (type: TVs and Limited Input devices). \
                             Enable the Gmail API, Google Calendar API, Google Drive API, \
                             and People API.",
                        ConnectorProvider::Microsoft =>
                            "Create an Azure AD App Registration → Authentication → \
                             Allow public client flows: Yes. Add Mail.ReadWrite and Mail.Send \
                             delegated permissions.",
                        ConnectorProvider::Coinbase =>
                            "Create a Coinbase Developer Platform app → OAuth settings → \
                             Add redirect URI http://127.0.0.1. Copy the Client ID and \
                             Client Secret.",
                        _ => "",
                    }
                })),
            )
                .into_response();
        }
    };

    let services_arr = body["services"].as_array();
    let has_service = |name: &str| -> bool {
        services_arr.is_none_or(|arr| arr.iter().any(|v| v.as_str() == Some(name)))
    };
    let services = ServicesConfig {
        communication: if has_service("communication") {
            Some(CommunicationConfig {
                enabled: true,
                from_address: None,
                folder: "INBOX".into(),
                poll_interval_secs: None,
                default_input_class: Default::default(),
                default_output_class: Default::default(),
                destination_rules: vec![],
                allowed_guild_ids: vec![],
                listen_channel_ids: vec![],
                default_send_channel_id: None,
            })
        } else {
            None
        },
        calendar: if has_service("calendar") {
            Some(CalendarConfig {
                enabled: true,
                default_class: Default::default(),
                resource_rules: vec![],
            })
        } else {
            None
        },
        drive: if has_service("drive") {
            Some(DriveConfig {
                enabled: true,
                default_class: Default::default(),
                resource_rules: vec![],
            })
        } else {
            None
        },
        contacts: if has_service("contacts") {
            Some(ContactsConfig {
                enabled: true,
                default_class: Default::default(),
                resource_rules: vec![],
            })
        } else {
            None
        },
        trading: if has_service("trading") {
            Some(hive_connectors::config::TradingConfig {
                enabled: true,
                default_input_class: Default::default(),
                default_output_class: Default::default(),
                sandbox: body.get("sandbox").and_then(|v| v.as_bool()).unwrap_or(false),
            })
        } else {
            None
        },
        custom: Default::default(),
    };
    let tmp_cfg = ConnectorConfig {
        id: channel_id.clone(),
        name: String::new(),
        provider,
        enabled: true,
        auth: AuthConfig::OAuth2 {
            client_id: client_id.clone(),
            client_secret: None,
            refresh_token: String::new(),
            access_token: None,
            token_url: None,
        },
        services,
        allowed_personas: Vec::new(),
    };
    let scopes = tmp_cfg.required_oauth_scopes().map(|v| v.join(" ")).unwrap_or_default();

    let client = shared_api_client();
    let email = body["email"].as_str().unwrap_or("").to_string();

    // Clear stale tokens so the poll endpoint doesn't short-circuit
    hive_connectors::secrets::delete(&channel_id, "access_token");
    hive_connectors::secrets::delete(&channel_id, "refresh_token");
    hive_connectors::secrets::delete(&channel_id, "client_secret");
    hive_connectors::secrets::delete(&channel_id, "oauth_error");

    match provider {
        ConnectorProvider::Gmail => {
            let client_secret = crate::provider_auth::resolve_client_secret(provider, &channel_id);
            let (port, code_rx) = match crate::provider_auth::start_oauth_callback_server().await {
                Ok(v) => v,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": e.to_string() })),
                    )
                        .into_response()
                }
            };
            let redirect_uri = format!("http://127.0.0.1:{port}");
            let state_token = uuid::Uuid::new_v4().to_string();
            let auth_url = crate::provider_auth::google_build_auth_url(
                &client_id,
                &redirect_uri,
                &state_token,
                &scopes,
            );

            let state_clone = state.clone();
            let channel_id_clone = channel_id.clone();
            let client_id_clone = client_id.clone();
            tokio::spawn(async move {
                match code_rx.await {
                    Ok(Ok(code)) => {
                        let client = shared_api_client();
                        match crate::provider_auth::google_exchange_auth_code(
                            client,
                            &client_id_clone,
                            &client_secret,
                            &code,
                            &redirect_uri,
                        )
                        .await
                        {
                            Ok(token_resp) => {
                                if let Some(err) = &token_resp.error {
                                    let desc =
                                        token_resp.error_description.as_deref().unwrap_or("");
                                    tracing::error!(
                                        error = %err,
                                        description = %desc,
                                        channel_id = %channel_id_clone,
                                        "Google OAuth token exchange returned error"
                                    );
                                    hive_connectors::secrets::save(
                                        &channel_id_clone,
                                        "oauth_error",
                                        &format!("{err}: {desc}"),
                                    );
                                } else if let Some(at) = &token_resp.access_token {
                                    hive_connectors::secrets::save(
                                        &channel_id_clone,
                                        "access_token",
                                        at,
                                    );
                                    if let Some(rt) = &token_resp.refresh_token {
                                        hive_connectors::secrets::save(
                                            &channel_id_clone,
                                            "refresh_token",
                                            rt,
                                        );
                                    }
                                    if !client_secret.is_empty() {
                                        hive_connectors::secrets::save(
                                            &channel_id_clone,
                                            "client_secret",
                                            &client_secret,
                                        );
                                    }

                                    save_oauth_channel(
                                        &state_clone,
                                        &channel_id_clone,
                                        ConnectorProvider::Gmail,
                                        &client_id_clone,
                                        &email,
                                    );
                                } else {
                                    tracing::error!(
                                        channel_id = %channel_id_clone,
                                        "Google OAuth token exchange returned no access_token and no error"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "failed to exchange Google auth code");
                                hive_connectors::secrets::save(
                                    &channel_id_clone,
                                    "oauth_error",
                                    &e.to_string(),
                                );
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "OAuth callback error");
                    }
                    Err(_) => {
                        tracing::warn!("OAuth callback server was dropped before receiving a code");
                    }
                }
            });

            Json(json!({
                "flow": "browser",
                "auth_url": auth_url,
                "channel_id": channel_id,
                "provider": "Gmail",
            }))
            .into_response()
        }
        ConnectorProvider::Microsoft => {
            let result =
                crate::provider_auth::outlook_device_code_request(client, &client_id, &scopes)
                    .await;
            match result {
                Ok(resp) => {
                    let mut pending = state.pending_device_codes.lock();
                    pending.insert(resp.device_code.clone(), resp.clone());
                    state.pending_oauth_meta.lock().insert(
                        resp.device_code.clone(),
                        OAuthPendingMeta {
                            channel_id: channel_id.clone(),
                            provider,
                            client_id,
                            email,
                        },
                    );
                    Json(json!({
                        "flow": "device_code",
                        "device_code": resp.device_code,
                        "user_code": resp.user_code,
                        "verification_uri": resp.verification_uri,
                        "expires_in": resp.expires_in,
                        "interval": resp.interval,
                        "provider": "Microsoft",
                    }))
                    .into_response()
                }
                Err(e) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
                        .into_response()
                }
            }
        }
        ConnectorProvider::Coinbase => {
            let client_secret = crate::provider_auth::resolve_client_secret(provider, &channel_id);
            let (port, code_rx) = match crate::provider_auth::start_oauth_callback_server().await {
                Ok(v) => v,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": e.to_string() })),
                    )
                        .into_response()
                }
            };
            let redirect_uri = format!("http://127.0.0.1:{port}");
            let state_token = uuid::Uuid::new_v4().to_string();
            let auth_url = format!(
                "https://login.coinbase.com/oauth2/auth\
                 ?response_type=code\
                 &client_id={client_id}\
                 &redirect_uri={redirect_uri}\
                 &scope={scopes}\
                 &state={state_token}",
                client_id = urlencoding::encode(&client_id),
                redirect_uri = urlencoding::encode(&redirect_uri),
                scopes = urlencoding::encode(&scopes),
                state_token = urlencoding::encode(&state_token),
            );

            let state_clone = state.clone();
            let channel_id_clone = channel_id.clone();
            let client_id_clone = client_id.clone();
            tokio::spawn(async move {
                match code_rx.await {
                    Ok(Ok(code)) => {
                        let client = shared_api_client();
                        match crate::provider_auth::exchange_auth_code_generic(
                            client,
                            "https://login.coinbase.com/oauth2/token",
                            &client_id_clone,
                            &client_secret,
                            &code,
                            &redirect_uri,
                        )
                        .await
                        {
                            Ok(token_resp) => {
                                if let Some(at) = &token_resp.access_token {
                                    hive_connectors::secrets::save(
                                        &channel_id_clone,
                                        "access_token",
                                        at,
                                    );
                                }
                                if let Some(rt) = &token_resp.refresh_token {
                                    hive_connectors::secrets::save(
                                        &channel_id_clone,
                                        "refresh_token",
                                        rt,
                                    );
                                }
                                if !client_secret.is_empty() {
                                    hive_connectors::secrets::save(
                                        &channel_id_clone,
                                        "client_secret",
                                        &client_secret,
                                    );
                                }

                                save_oauth_channel(
                                    &state_clone,
                                    &channel_id_clone,
                                    ConnectorProvider::Coinbase,
                                    &client_id_clone,
                                    &email,
                                );

                                state_clone.pending_oauth_meta.lock().insert(
                                    channel_id_clone.clone(),
                                    OAuthPendingMeta {
                                        channel_id: channel_id_clone,
                                        provider: ConnectorProvider::Coinbase,
                                        client_id: client_id_clone,
                                        email,
                                    },
                                );
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "failed to exchange Coinbase auth code");
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "Coinbase OAuth callback error");
                    }
                    Err(_) => {
                        tracing::warn!(
                            "Coinbase OAuth callback server was dropped before receiving a code"
                        );
                    }
                }
            });

            Json(json!({
                "flow": "browser",
                "auth_url": auth_url,
                "channel_id": channel_id,
                "provider": "Coinbase",
            }))
            .into_response()
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "OAuth device code flow not supported for this provider" })),
        )
            .into_response(),
    }
}

/// Helper: auto-create or update a connector config after successful OAuth.
fn save_oauth_channel(
    state: &AppState,
    channel_id: &str,
    _provider: hive_contracts::connectors::ConnectorProvider,
    client_id: &str,
    email: &str,
) {
    let connectors_path = state.hivemind_home.join("connectors.yaml");
    let mut configs: Vec<hive_connectors::ConnectorConfig> =
        std::fs::read_to_string(&connectors_path)
            .ok()
            .and_then(|yaml| serde_yaml::from_str(&yaml).ok())
            .unwrap_or_default();

    if let Some(existing) = configs.iter_mut().find(|c| c.id == channel_id) {
        if let hive_connectors::config::AuthConfig::OAuth2 {
            client_id: ref mut cfg_client_id,
            ..
        } = existing.auth
        {
            *cfg_client_id = client_id.to_string();
        }
        if !email.is_empty() {
            if let Some(ref mut comm) = existing.services.communication {
                comm.from_address = Some(email.to_string());
            }
        }

        if let Ok(yaml) = serde_yaml::to_string(&configs) {
            let _ = std::fs::write(&connectors_path, &yaml);
        }
        if let Some(connectors) = &state.connectors {
            let _ = connectors.load_connectors(configs);
        }
    } else {
        tracing::debug!(
            connector_id = %channel_id,
            "save_oauth_channel: connector not found in connectors.yaml — \
             skipping reload (wizard may not have finished yet)"
        );
    }
}

/// Poll for OAuth completion.
pub(crate) async fn api_channel_oauth_poll(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    use hive_contracts::connectors::ConnectorProvider;

    let flow = body["flow"].as_str().unwrap_or("device_code");

    if flow == "browser" {
        let has_token = hive_connectors::secrets::load(&channel_id, "refresh_token")
            .filter(|s| !s.is_empty())
            .is_some();

        if has_token {
            // Clean up any stale error
            hive_connectors::secrets::delete(&channel_id, "oauth_error");
            state.pending_oauth_meta.lock().remove(&channel_id);
            return Json(json!({
                "status": "complete",
                "channel_id": channel_id,
            }))
            .into_response();
        }
        // Check if the token exchange reported an error
        if let Some(oauth_err) =
            hive_connectors::secrets::load(&channel_id, "oauth_error").filter(|s| !s.is_empty())
        {
            hive_connectors::secrets::delete(&channel_id, "oauth_error");
            return Json(json!({ "status": "failed", "error": oauth_err })).into_response();
        }
        return Json(json!({ "status": "pending" })).into_response();
    }

    let device_code = match body["device_code"].as_str() {
        Some(dc) => dc.to_string(),
        None => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "missing device_code" })))
                .into_response()
        }
    };

    let meta = match state.pending_oauth_meta.lock().get(&device_code).cloned() {
        Some(m) => m,
        None => {
            let has_token = hive_connectors::secrets::load(&channel_id, "access_token")
                .filter(|s| !s.is_empty())
                .is_some();
            if has_token {
                return Json(json!({
                    "status": "complete",
                    "channel_id": channel_id,
                }))
                .into_response();
            }
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "no pending OAuth flow for this device_code" })),
            )
                .into_response();
        }
    };

    let already_done = hive_connectors::secrets::load(&channel_id, "access_token")
        .filter(|s| !s.is_empty())
        .is_some();
    if already_done {
        state.pending_device_codes.lock().remove(&device_code);
        state.pending_oauth_meta.lock().remove(&device_code);
        return Json(json!({
            "status": "complete",
            "channel_id": channel_id,
        }))
        .into_response();
    }

    let client = shared_api_client();
    let poll_result = match meta.provider {
        ConnectorProvider::Microsoft => {
            crate::provider_auth::outlook_poll_for_token(client, &meta.client_id, &device_code)
                .await
        }
        ConnectorProvider::Gmail => {
            let client_secret =
                crate::provider_auth::resolve_client_secret(ConnectorProvider::Gmail, &channel_id);
            crate::provider_auth::google_poll_for_token(
                client,
                &meta.client_id,
                &client_secret,
                &device_code,
            )
            .await
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "device code polling not supported for this provider" })),
            )
                .into_response()
        }
    };

    match poll_result {
        Ok(resp) => {
            if let Some(new_access_token) = &resp.access_token {
                let new_refresh_token = resp.refresh_token.clone().unwrap_or_default();

                hive_connectors::secrets::save(&channel_id, "access_token", new_access_token);
                if !new_refresh_token.is_empty() {
                    hive_connectors::secrets::save(
                        &channel_id,
                        "refresh_token",
                        &new_refresh_token,
                    );
                }

                // Google needs client_secret for token refresh
                if meta.provider == ConnectorProvider::Gmail {
                    let client_secret = crate::provider_auth::resolve_client_secret(
                        ConnectorProvider::Gmail,
                        &channel_id,
                    );
                    if !client_secret.is_empty() {
                        hive_connectors::secrets::save(
                            &channel_id,
                            "client_secret",
                            &client_secret,
                        );
                    }
                }

                save_oauth_channel(
                    &state,
                    &channel_id,
                    meta.provider,
                    &meta.client_id,
                    &meta.email,
                );

                state.pending_device_codes.lock().remove(&device_code);
                state.pending_oauth_meta.lock().remove(&device_code);

                Json(json!({
                    "status": "complete",
                    "channel_id": channel_id,
                }))
                .into_response()
            } else if resp.error.as_deref() == Some("authorization_pending") {
                Json(json!({ "status": "pending" })).into_response()
            } else if resp.error.as_deref() == Some("slow_down") {
                Json(json!({ "status": "pending", "slow_down": true })).into_response()
            } else {
                let error = resp
                    .error_description
                    .or(resp.error)
                    .unwrap_or_else(|| "unknown error".to_string());
                Json(json!({ "status": "failed", "error": error })).into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
            .into_response(),
    }
}

/// Send a message through a connector (called by comm tools).
pub(crate) async fn api_comm_send(
    State(state): State<AppState>,
    Json(req): Json<Value>,
) -> impl IntoResponse {
    let connectors = match &state.connectors {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "connector service not initialized" })),
            )
                .into_response()
        }
    };

    let connector_id = match req.get("connector_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "missing connector_id" })))
                .into_response()
        }
    };

    let to: Vec<String> = match req.get("to") {
        Some(Value::Array(arr)) => {
            arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
        }
        Some(Value::String(s)) => vec![s.clone()],
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "missing to" })))
                .into_response()
        }
    };

    let body = match req.get("body").and_then(|v| v.as_str()) {
        Some(b) => b.to_string(),
        None => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "missing body" })))
                .into_response()
        }
    };

    let subject = req.get("subject").and_then(|v| v.as_str()).map(|s| s.to_string());
    let agent_id = req.get("agent_id").and_then(|v| v.as_str()).map(|s| s.to_string());
    let session_id = req.get("session_id").and_then(|v| v.as_str()).map(|s| s.to_string());

    match connectors
        .send_message(
            &connector_id,
            &to,
            subject.as_deref(),
            &body,
            &[],
            agent_id.as_deref(),
            session_id.as_deref(),
        )
        .await
    {
        Ok(msg) => {
            Json(serde_json::to_value(&msg).unwrap_or(json!({ "status": "sent" }))).into_response()
        }
        Err(e) => {
            tracing::warn!(connector = %connector_id, "comm send failed: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("{e:#}") })))
                .into_response()
        }
    }
}

/// Read new messages from a connector.
pub(crate) async fn api_comm_read(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let connectors = match &state.connectors {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "connector service not initialized" })),
            )
                .into_response()
        }
    };

    let connector_id = match params.get("connector_id") {
        Some(id) => id.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "missing connector_id query param" })),
            )
                .into_response()
        }
    };

    let limit: usize = params.get("limit").and_then(|l| l.parse().ok()).unwrap_or(20);

    let agent_id = params.get("agent_id").cloned();
    let session_id = params.get("session_id").cloned();

    match connectors
        .read_messages(&connector_id, limit, agent_id.as_deref(), session_id.as_deref())
        .await
    {
        Ok(msgs) => Json(json!({ "messages": msgs })).into_response(),
        Err(e) => {
            tracing::warn!(connector = %connector_id, "comm read failed: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("{e:#}") })))
                .into_response()
        }
    }
}

/// Query the connector audit log.
pub(crate) async fn api_comm_audit(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let connectors = match &state.connectors {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "connector service not initialized" })),
            )
                .into_response()
        }
    };

    let filter = hive_connectors::audit::ConnectorAuditFilter {
        connector_id: params.get("connector_id").cloned(),
        service_type: params.get("service_type").map(|s| match s.as_str() {
            "communication" => hive_contracts::connectors::ServiceType::Communication,
            "calendar" => hive_contracts::connectors::ServiceType::Calendar,
            "drive" => hive_contracts::connectors::ServiceType::Drive,
            "contacts" => hive_contracts::connectors::ServiceType::Contacts,
            other => hive_contracts::connectors::ServiceType::Other(other.to_string()),
        }),
        direction: params.get("direction").and_then(|d| match d.as_str() {
            "inbound" => Some(hive_contracts::comms::MessageDirection::Inbound),
            "outbound" => Some(hive_contracts::comms::MessageDirection::Outbound),
            _ => None,
        }),
        agent_id: params.get("agent_id").cloned(),
        since_ms: params.get("since_ms").and_then(|s| s.parse().ok()),
        until_ms: params.get("until_ms").and_then(|s| s.parse().ok()),
        limit: params.get("limit").and_then(|l| l.parse().ok()),
    };

    match connectors.search_audit(&filter) {
        Ok(entries) => Json(json!({ "results": entries })).into_response(),
        Err(e) => {
            tracing::warn!("connector audit query failed: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("{e:#}") })))
                .into_response()
        }
    }
}
