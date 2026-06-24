use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use relais_core::redact::{secret_values_of, Redactor};
use relais_core::types::{Credentials, ExecContext};
use relais_core::AdapterError;
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
    let adapter = state.router.get(&site).ok_or(StatusCode::NOT_FOUND)?;

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

    let adapter = state.router.get(site_id).ok_or(StatusCode::NOT_FOUND)?;

    let resources = adapter.resources();
    let action = find_action(&resources, resource_id, action_id).ok_or(StatusCode::NOT_FOUND)?;

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
pub async fn exec_action(State(state): State<AppState>, Json(body): Json<ExecRequest>) -> Response {
    // Existence check for a clean 404 (router.exec would otherwise map a missing
    // site to a 500). Use the same structured error shape as every other error.
    if state.router.get(&body.site).is_none() {
        return error_response(&AdapterError::SiteNotFound(body.site.clone()), &None);
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
            match relais_core::token_refresh::maybe_refresh(&cred, &body.site, state.vault.as_ref())
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
        Ok(response) => Json(response).into_response(),
        Err(err) => error_response(&err, &ctx.credentials),
    }
}

/// Map an adapter error to (HTTP status, stable `kind`).
fn classify(err: &AdapterError) -> (StatusCode, &'static str) {
    match err {
        AdapterError::Auth(_) => (StatusCode::UNAUTHORIZED, "auth"),
        AdapterError::RateLimited { .. } => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
        AdapterError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        AdapterError::Unsupported(_) => (StatusCode::BAD_REQUEST, "unsupported"),
        AdapterError::SiteNotFound(_) => (StatusCode::NOT_FOUND, "site_not_found"),
        AdapterError::AuditUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "audit_unavailable"),
        AdapterError::Upstream(_) => (StatusCode::BAD_GATEWAY, "upstream"),
        AdapterError::Other(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    }
}

/// The error text with this request's credential values masked. Upstream error text
/// can echo a request credential, so it must not be returned to the caller verbatim
/// (the audit redaction boundary must not be bypassed on the error path).
fn redacted_message(err: &AdapterError, credentials: &Option<Credentials>) -> String {
    Redactor::new().redact_str(&err.to_string(), &secret_values_of(credentials))
}

/// Build a safe HTTP response for an adapter error: proper status, a structured
/// redacted body, and a `Retry-After` header for rate limits.
fn error_response(err: &AdapterError, credentials: &Option<Credentials>) -> Response {
    let (status, kind) = classify(err);
    let message = redacted_message(err, credentials);
    let body = Json(json!({ "error": { "kind": kind, "message": message } }));
    let mut resp = (status, body).into_response();
    if let AdapterError::RateLimited { retry_after_secs } = err {
        if let Ok(v) = HeaderValue::from_str(&retry_after_secs.to_string()) {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_request_credential_in_error_message() {
        // An error whose text echoes the request's API token must be masked.
        let creds = Some(Credentials::api_key("SECRET_TOKEN_abc"));
        let err = AdapterError::Auth("upstream said: token SECRET_TOKEN_abc bad".into());
        let msg = redacted_message(&err, &creds);
        assert!(!msg.contains("SECRET_TOKEN_abc"), "token leaked: {msg}");
    }

    #[test]
    fn maps_known_variants_to_statuses() {
        let cases = [
            (
                AdapterError::Auth("x".into()),
                StatusCode::UNAUTHORIZED,
                "auth",
            ),
            (
                AdapterError::NotFound("x".into()),
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            (
                AdapterError::Unsupported("x".into()),
                StatusCode::BAD_REQUEST,
                "unsupported",
            ),
            (
                AdapterError::RateLimited {
                    retry_after_secs: 1,
                },
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
            ),
            (
                AdapterError::SiteNotFound("x".into()),
                StatusCode::NOT_FOUND,
                "site_not_found",
            ),
        ];
        for (err, status, kind) in cases {
            let (got_status, got_kind) = classify(&err);
            assert_eq!(got_status, status, "status for {err:?}");
            assert_eq!(got_kind, kind, "kind for {err:?}");
        }
    }

    #[test]
    fn rate_limited_sets_retry_after_header() {
        let resp = error_response(
            &AdapterError::RateLimited {
                retry_after_secs: 42,
            },
            &None,
        );
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers().get(header::RETRY_AFTER).unwrap(),
            "42",
            "Retry-After header"
        );
    }
}
