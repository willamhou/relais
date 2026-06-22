//! Mapping from relais `ExecContext`/`Response` to the signet [`Action`] +
//! response-content envelope that the writer signs (C2).
//!
//! The redacted request value becomes **both** `Action.params` and the sidecar's
//! `request`; the response envelope becomes **both** the `sign_compound`
//! `response_content` (hashed) and the sidecar's `response`. Keeping them identical
//! is what lets `relais audit verify` recompute the hashes (C6).

use serde_json::{json, Map, Value};
use signet_core::Action;

use super::redact::{AuditMeta, Redactor};
use crate::error::AdapterError;
use crate::types::{ExecContext, Response};

/// Build the redacted request envelope: `redact(ctx.params)` with `_relais_audit`
/// metadata attached. If `ctx.params` is not a JSON object it is nested under a
/// `"params"` key so the audit metadata always has a home.
pub fn build_request(
    ctx: &ExecContext,
    meta: &AuditMeta,
    redactor: &Redactor,
    secrets: &[String],
) -> Value {
    let redacted = redactor.redact_value(&ctx.params, secrets);
    let mut obj = match redacted {
        // Caller already uses `_relais_audit` → nest its params so our metadata is
        // unambiguous and nothing is silently overwritten.
        Value::Object(m) if m.contains_key("_relais_audit") => {
            let mut wrap = Map::new();
            wrap.insert("params".to_string(), Value::Object(m));
            wrap
        }
        Value::Object(m) => m,
        other => {
            let mut m = Map::new();
            m.insert("params".to_string(), other);
            m
        }
    };
    obj.insert("_relais_audit".to_string(), meta.to_json());
    Value::Object(obj)
}

/// Build the response-content envelope that `sign_compound` hashes. The success/
/// failure outcome lives here (signet's `sign_compound` leaves `Response.outcome`
/// as `None`). `transport_ok` is explicitly transport-level — a business error
/// carried in a 2xx body is still `transport_ok: true`, with the body in `data`.
pub fn build_response_envelope(
    result: &Result<Response, AdapterError>,
    redactor: &Redactor,
    secrets: &[String],
) -> Value {
    match result {
        Ok(resp) => json!({
            "transport_ok": true,
            "data": redactor.redact_value(&resp.data, secrets),
            "business_status": "unclassified",
        }),
        Err(e) => json!({
            "transport_ok": false,
            "error": {
                "kind": error_kind(e),
                // redact: error text can echo a credential (BLOCKER)
                "message": redactor.redact_str(&e.to_string(), secrets),
            },
        }),
    }
}

/// Construct the signet [`Action`]. `params_hash` is left empty — `sign_compound`
/// computes it. `target` is exactly the site's `base_url` (the site id is already in
/// `tool`); the resolved endpoint path lives in adapter code and is not attested.
pub fn build_action(
    ctx: &ExecContext,
    request: Value,
    base_url: &str,
    session: Option<String>,
    trace_id: String,
    call_id: String,
) -> Action {
    Action {
        tool: format!("{}.{}.{}", ctx.site, ctx.resource, ctx.action),
        params: request,
        params_hash: String::new(),
        target: base_url.to_string(),
        transport: "https".to_string(),
        session,
        call_id: Some(call_id),
        response_hash: None,
        trace_id: Some(trace_id),
        parent_receipt_id: None,
    }
}

/// Stable short identifier for an `AdapterError` variant (the `kind` in a failure
/// envelope).
fn error_kind(e: &AdapterError) -> &'static str {
    match e {
        AdapterError::Auth(_) => "auth",
        AdapterError::RateLimited { .. } => "rate_limited",
        AdapterError::NotFound(_) => "not_found",
        AdapterError::Unsupported(_) => "unsupported",
        AdapterError::SiteNotFound(_) => "site_not_found",
        AdapterError::AuditUnavailable(_) => "audit_unavailable",
        AdapterError::Upstream(_) => "upstream",
        AdapterError::Other(_) => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Credentials, ResponseMeta};
    use serde_json::json;

    fn meta() -> AuditMeta {
        AuditMeta {
            auth_injection: "acs_token->query".into(),
            credential_ref: "kref_test".into(),
            t0: "2026-06-21T00:00:00Z".into(),
            t1: "2026-06-21T00:00:01Z".into(),
        }
    }

    fn ctx_with_token(tok: &str) -> ExecContext {
        ExecContext {
            site: "scs".into(),
            resource: "order".into(),
            action: "create".into(),
            params: json!({ "customer_id": "42", "note": tok }),
            credentials: Some(Credentials::api_key(tok)),
        }
    }

    #[test]
    fn request_envelope_has_audit_meta_and_redacts() {
        let r = Redactor::new();
        let ctx = ctx_with_token("TOK123");
        let secrets = super::super::redact::secret_values_of(&ctx.credentials);
        let req = build_request(&ctx, &meta(), &r, &secrets);
        assert_eq!(req["customer_id"], json!("42"));
        // token echoed in a non-sensitive key is masked by value
        assert_eq!(req["note"], json!(super::super::redact::REDACTED));
        assert_eq!(
            req["_relais_audit"]["auth_injection"],
            json!("acs_token->query")
        );
        assert_eq!(req["_relais_audit"]["t0"], json!("2026-06-21T00:00:00Z"));
    }

    #[test]
    fn build_action_uses_real_signet_fields() {
        let r = Redactor::new();
        let ctx = ctx_with_token("TOK123");
        let secrets = super::super::redact::secret_values_of(&ctx.credentials);
        let req = build_request(&ctx, &meta(), &r, &secrets);
        let a = build_action(
            &ctx,
            req,
            "https://api.example",
            Some("sub".into()),
            "trace".into(),
            "call".into(),
        );
        assert_eq!(a.tool, "scs.order.create");
        assert_eq!(a.target, "https://api.example");
        assert_eq!(a.transport, "https");
        assert_eq!(a.session.as_deref(), Some("sub"));
        assert_eq!(a.call_id.as_deref(), Some("call"));
        assert_eq!(a.trace_id.as_deref(), Some("trace"));
        assert!(a.parent_receipt_id.is_none());
        assert!(a.params_hash.is_empty());
    }

    #[test]
    fn response_envelope_ok_and_err() {
        let r = Redactor::new();
        let ok: Result<Response, AdapterError> = Ok(Response {
            data: json!({ "x": 1 }),
            meta: ResponseMeta::default(),
        });
        let env = build_response_envelope(&ok, &r, &[]);
        assert_eq!(env["transport_ok"], json!(true));
        assert_eq!(env["data"]["x"], json!(1));

        let err: Result<Response, AdapterError> = Err(AdapterError::NotFound("nope".into()));
        let env = build_response_envelope(&err, &r, &[]);
        assert_eq!(env["transport_ok"], json!(false));
        assert_eq!(env["error"]["kind"], json!("not_found"));
    }

    #[test]
    fn error_message_is_redacted() {
        let r = Redactor::new();
        let tok = "ERRTOKEN_xyz";
        let err: Result<Response, AdapterError> =
            Err(AdapterError::Auth(format!("bad token {tok}")));
        let env = build_response_envelope(&err, &r, &[tok.to_string()]);
        let s = serde_json::to_string(&env).unwrap();
        assert!(!s.contains(tok), "secret leaked via error message: {s}");
        assert_eq!(env["error"]["kind"], json!("auth"));
    }

    #[test]
    fn relais_audit_key_collision_is_nested_not_overwritten() {
        let r = Redactor::new();
        let ctx = ExecContext {
            site: "s".into(),
            resource: "r".into(),
            action: "a".into(),
            params: json!({ "_relais_audit": "caller-value", "x": 1 }),
            credentials: None,
        };
        let req = build_request(&ctx, &meta(), &r, &[]);
        // our metadata wins at top level; caller's data preserved under "params"
        assert_eq!(req["_relais_audit"]["credential_ref"], json!("kref_test"));
        assert_eq!(req["params"]["_relais_audit"], json!("caller-value"));
        assert_eq!(req["params"]["x"], json!(1));
    }

    /// The leak guard: a credential token echoed in both a request param and the
    /// response body must not survive into the signed Action or the response envelope.
    #[test]
    fn secret_leak_guard_request_and_response() {
        let r = Redactor::new();
        let tok = "SUPER_SECRET_TOKEN_value";
        let ctx = ctx_with_token(tok);
        let secrets = super::super::redact::secret_values_of(&ctx.credentials);

        let req = build_request(&ctx, &meta(), &r, &secrets);
        let action = build_action(
            &ctx,
            req,
            "https://api.example",
            None,
            "t".into(),
            "c".into(),
        );
        let action_json = serde_json::to_string(&action).unwrap();
        assert!(
            !action_json.contains(tok),
            "token leaked into signed Action"
        );

        let resp: Result<Response, AdapterError> = Ok(Response {
            data: json!({ "login": { "acs_token": tok }, "echo": tok }),
            meta: ResponseMeta::default(),
        });
        let env = build_response_envelope(&resp, &r, &secrets);
        let env_json = serde_json::to_string(&env).unwrap();
        assert!(
            !env_json.contains(tok),
            "token leaked into response envelope"
        );
    }
}
