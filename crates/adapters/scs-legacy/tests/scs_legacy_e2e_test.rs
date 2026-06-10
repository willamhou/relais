//! End-to-end test against a *live* legacy SCS instance (`:8501`).
//!
//! Ignored by default — standing up the legacy Beego app + its database is
//! heavyweight. To run it, start legacy SCS and point the adapter at it:
//!
//! ```sh
//! SCS_LEGACY_BASE_URL=http://127.0.0.1:8501 \
//!   cargo test -p relais-adapter-scs-legacy --test scs_legacy_e2e_test -- --ignored
//! ```
//!
//! This verifies the full chain — adapter → spec lookup → URL build → request →
//! real legacy router → JSON response parsing — against a real server. It asserts
//! the adapter reaches the legacy router and passes its business JSON back through
//! the relais envelope; it does not assert business success (legacy seed data may
//! lag the code schema).
use relais_adapter_scs_legacy::ScsLegacyAdapter;
use relais_core::{Adapter, ExecContext};
use serde_json::json;

fn live_adapter() -> ScsLegacyAdapter {
    let base = std::env::var("SCS_LEGACY_BASE_URL")
        .expect("set SCS_LEGACY_BASE_URL to a live legacy SCS instance to run this");
    ScsLegacyAdapter::with_base_url(base)
}

#[tokio::test]
#[ignore = "requires a live legacy SCS; set SCS_LEGACY_BASE_URL and run with --ignored"]
async fn e2e_login_endpoint_round_trips() {
    let adapter = live_adapter();
    let ctx = ExecContext {
        site: "scs".into(),
        resource: "login".into(),
        action: "do".into(),
        params: json!({"login_name": "admin", "password": "admin"}),
        credentials: None,
    };

    // We reached the real legacy router (POST /1/login/do) and got a JSON business
    // response back; the adapter passes it through verbatim into Response.data.
    let resp = adapter.exec(&ctx).await.expect("legacy should respond");
    assert!(
        resp.data.is_object(),
        "expected a JSON object from legacy, got {:?}",
        resp.data
    );
    // A legacy business response is either a success payload or {err_code, err_msg};
    // either way the chain (lookup -> URL -> POST -> parse) worked.
    let obj = resp.data.as_object().unwrap();
    assert!(
        obj.contains_key("err_code") || obj.contains_key("data") || obj.contains_key("acs_token"),
        "unexpected legacy response shape: {:?}",
        resp.data
    );
}
