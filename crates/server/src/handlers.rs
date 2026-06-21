use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use relais_core::types::{Credentials, ExecContext};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state::AppState;

/// GET /health — returns "ok", no auth required.
pub async fn health() -> &'static str {
    "ok"
}

/// GET /v1/sites — list all registered site manifests.
pub async fn list_sites(State(state): State<AppState>) -> impl IntoResponse {
    let sites = state.router.sites();
    Json(sites)
}

/// GET /v1/apis/:site — list resources for a given site.
pub async fn list_apis(
    State(state): State<AppState>,
    Path(site): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let adapter = state
        .router
        .get(&site)
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(adapter.resources()))
}

/// GET /v1/spec/:spec_path — get action spec.
///
/// The spec_path is dot-delimited: "site.resource.action"
pub async fn get_spec(
    State(state): State<AppState>,
    Path(spec_path): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let parts: Vec<&str> = spec_path.splitn(3, '.').collect();
    if parts.len() != 3 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let (site_id, resource_id, action_id) = (parts[0], parts[1], parts[2]);

    let adapter = state
        .router
        .get(site_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    let resources = adapter.resources();
    let action = find_action(&resources, resource_id, action_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(action))
}

/// Recursively search resources for a matching action.
fn find_action(
    resources: &[relais_core::types::Resource],
    resource_id: &str,
    action_id: &str,
) -> Option<relais_core::types::Action> {
    for resource in resources {
        if resource.id == resource_id {
            for action in &resource.actions {
                if action.id == action_id {
                    return Some(action.clone());
                }
            }
        }
        // Search children recursively
        if let Some(action) = find_action(&resource.children, resource_id, action_id) {
            return Some(action);
        }
    }
    None
}

/// Request body for POST /v1/exec.
#[derive(Debug, Deserialize)]
pub struct ExecRequest {
    pub site: String,
    pub resource: String,
    pub action: String,
    #[serde(default)]
    pub params: Value,
}

/// POST /v1/exec — execute an action via the router.
pub async fn exec_action(
    State(state): State<AppState>,
    Json(body): Json<ExecRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    // Existence check for a clean 404 (router.exec would otherwise map a missing
    // site to a 500). The actual execution goes through the router choke point.
    if state.router.get(&body.site).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("site '{}' not found", body.site)})),
        ));
    }

    // Look up credentials from vault for this site.
    let credentials = state.vault.as_ref().and_then(|vault| {
        vault
            .retrieve(&body.site)
            .ok()
            .flatten()
            .and_then(|json_str| {
                serde_json::from_str::<Credentials>(&json_str)
                    .ok()
                    .or_else(|| Some(Credentials::api_key(&json_str)))
            })
    });

    // If the token is expired, try to refresh it automatically.
    let credentials = if let Some(cred) = credentials {
        if cred.is_expired() {
            match relais_core::token_refresh::maybe_refresh(
                &cred,
                &body.site,
                state.vault.as_ref(),
            )
            .await
            {
                Ok(refreshed) => Some(refreshed),
                Err(e) => {
                    tracing::warn!("token refresh failed for {}: {}", body.site, e);
                    Some(cred)
                }
            }
        } else {
            Some(cred)
        }
    } else {
        None
    };

    // Warn if cookie credentials are stale.
    if let Some(ref cred) = credentials {
        if cred.is_cookie_stale(24) {
            tracing::warn!(
                "cookies for {} are older than 24 hours, consider re-authenticating \
                 with 'relais auth import-cookies'",
                body.site
            );
        }
    }

    let ctx = ExecContext {
        site: body.site,
        resource: body.resource,
        action: body.action,
        params: body.params,
        credentials,
    };

    match state.router.exec(&ctx).await {
        Ok(response) => Ok(Json(response)),
        Err(err) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": err.to_string()})),
        )),
    }
}
