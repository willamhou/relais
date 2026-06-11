//! L-C — write-path reachability against live legacy.
//!
//! Proves the adapter's WRITE chain reaches legacy's business layer, without
//! mutating any data: it probes create/add/save actions on core modules with
//! ONLY the `acs_token` and NO business params. Legacy rejects them at the
//! business layer (permission or required-field validation), which is exactly
//! what we assert — the request routed, was authenticated, and ran business
//! logic. Missing required params guarantee nothing is ever created.
//!
//! Ignored by default; needs a live legacy at SCS_LEGACY_BASE_URL with an
//! aligned schema (see generate/schema_sync.py) so login + business logic run.
//!
//! ```sh
//! SCS_LEGACY_BASE_URL=http://127.0.0.1:8501 \
//!   cargo test -p relais-adapter-scs-legacy --test scs_legacy_writepath_test -- --ignored --nocapture
//! ```
use relais_adapter_scs_legacy::ScsLegacyAdapter;
use relais_core::{Adapter, Credentials, ExecContext};
use serde_json::{json, Value};

const CORE_MODULES: &[&str] = &[
    "accounts",
    "goods",
    "customers",
    "suppliers",
    "products",
    "product_categories",
    "orders",
];
// Create-style verbs only — with no params these hit required-field validation,
// so they never actually write. (Deliberately excludes delete/update verbs.)
const CREATE_VERBS: &[&str] = &["create", "add", "save"];

fn ctx(resource: &str, action: &str, params: Value, token: &str) -> ExecContext {
    ExecContext {
        site: "scs".into(),
        resource: resource.into(),
        action: action.into(),
        params,
        credentials: Some(Credentials::api_key(token)),
    }
}

async fn login(adapter: &ScsLegacyAdapter) -> String {
    let resp = adapter
        .exec(&ExecContext {
            site: "scs".into(),
            resource: "login".into(),
            action: "do".into(),
            params: json!({"login_name": "admin", "password": "admin"}),
            credentials: None,
        })
        .await
        .expect("login.do should respond");
    resp.data
        .get("acs_token")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("login did not return acs_token: {:?}", resp.data))
        .to_string()
}

#[tokio::test]
#[ignore = "requires a live legacy SCS with aligned schema; set SCS_LEGACY_BASE_URL"]
async fn write_path_reaches_business_layer() {
    let base = std::env::var("SCS_LEGACY_BASE_URL")
        .expect("set SCS_LEGACY_BASE_URL to a live legacy SCS instance to run this");
    let adapter = ScsLegacyAdapter::with_base_url(base);
    let token = login(&adapter).await;

    let targets: Vec<(String, String)> = adapter
        .resources()
        .iter()
        .filter(|r| CORE_MODULES.contains(&r.id.as_str()))
        .flat_map(|r| {
            let rid = r.id.clone();
            r.actions
                .iter()
                .filter(|a| {
                    let id = a.id.to_lowercase();
                    CREATE_VERBS.iter().any(|v| id.contains(v))
                })
                .map(move |a| (rid.clone(), a.id.clone()))
        })
        .collect();

    assert!(
        !targets.is_empty(),
        "expected some create-style write endpoints"
    );

    let mut reachable = 0usize;
    let mut not_reachable: Vec<String> = Vec::new();
    for (rid, aid) in &targets {
        // token only, NO business params -> business-layer validation rejects it.
        let result = adapter.exec(&ctx(rid, aid, json!({}), &token)).await;
        let ok = match result {
            Ok(resp) => {
                let msg = resp
                    .data
                    .get("err_msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let raw = resp.data.to_string();
                // reachable = routed + ran business logic (not "route not found",
                // not a schema/dependency system error)
                msg != "请求的服务不存在"
                    && !msg.contains("系统异常")
                    && !raw.contains("pq:")
                    && !raw.contains("does not exist")
            }
            Err(_) => false,
        };
        if ok {
            reachable += 1;
        } else {
            not_reachable.push(format!("{rid}.{aid}"));
        }
    }

    println!("\n=== write-path reachability (core modules, create-style) ===");
    println!("write endpoints probed : {}", targets.len());
    println!("business-reachable     : {reachable}");
    println!("not reachable          : {}", not_reachable.len());
    for n in not_reachable.iter().take(30) {
        println!("  {n}");
    }

    // The write chain (route + auth + business validation) should work for the
    // vast majority of core create endpoints. Allow a small tail.
    let rate = reachable as f64 / targets.len() as f64;
    assert!(
        rate >= 0.85,
        "only {}/{} write endpoints reached the business layer ({:.0}%)",
        reachable,
        targets.len(),
        rate * 100.0
    );
}
