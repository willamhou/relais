//! L-B — business reachability sweep against live legacy with a REAL token.
//!
//! Logs in (admin/admin), then probes READ-ONLY endpoints with the real
//! `acs_token`, classifying each response:
//!
//! - routed-miss: legacy "route not found" (should be none; see L2)
//! - system-error: schema/dependency failure ("系统异常", "pq:", "does not exist")
//! - business-reachable: success OR a business-level param error — i.e. the
//!   business logic actually executed (what we want)
//!
//! This measures how much of the API truly runs its business logic, beyond L2's
//! route check. Read-only = GET methods + actions whose name looks like a query
//! (list/get/query/detail/info/show/page/find/...), so we never trigger writes.
//!
//! Ignored by default; needs a live legacy at SCS_LEGACY_BASE_URL with a schema
//! aligned to the code (see generate/schema_sync notes) so login succeeds.
//!
//! ```sh
//! SCS_LEGACY_BASE_URL=http://127.0.0.1:8501 \
//!   cargo test -p relais-adapter-scs-legacy --test scs_legacy_business_test -- --ignored --nocapture
//! ```
use futures::stream::{self, StreamExt};
use relais_adapter_scs_legacy::ScsLegacyAdapter;
use relais_core::{Adapter, ExecContext, Method};
use serde_json::{json, Value};

use std::time::Duration;

const CONCURRENCY: usize = 12;
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const READ_KW: &[&str] = &[
    "list", "get", "query", "detail", "info", "show", "page", "find", "count", "tree", "stat",
    "check", "index",
];
// Heavyweight export/report modules — slow and not representative business queries.
const SKIP_MODULES: &[&str] = &["reports", "print", "statistics"];

fn is_read_only(method: &Method, action: &str) -> bool {
    if matches!(method, Method::Read) {
        return true; // GET
    }
    let a = action.to_lowercase();
    READ_KW.iter().any(|k| a.contains(k))
}

fn ctx(resource: &str, action: &str, params: Value, token: Option<&str>) -> ExecContext {
    ExecContext {
        site: "scs".into(),
        resource: resource.into(),
        action: action.into(),
        params,
        credentials: token.map(relais_core::Credentials::api_key),
    }
}

async fn login(adapter: &ScsLegacyAdapter) -> String {
    let resp = adapter
        .exec(&ctx(
            "login",
            "do",
            json!({"login_name": "admin", "password": "admin"}),
            None,
        ))
        .await
        .expect("login.do should respond");
    resp.data
        .get("acs_token")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("login did not return acs_token: {:?}", resp.data))
        .to_string()
}

enum Class {
    RoutedMiss,
    SystemError(String),
    BusinessReachable,
    Slow,
    Transport,
}

fn classify(result: Result<relais_core::Response, relais_core::AdapterError>) -> Class {
    match result {
        Ok(resp) => {
            let raw = resp.data.to_string();
            let msg = resp
                .data
                .get("err_msg")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if msg == "请求的服务不存在" {
                Class::RoutedMiss
            } else if msg.contains("系统异常")
                || raw.contains("pq:")
                || raw.contains("does not exist")
            {
                Class::SystemError(msg.to_string())
            } else {
                Class::BusinessReachable
            }
        }
        Err(relais_core::AdapterError::NotFound(_)) => Class::RoutedMiss,
        Err(_) => Class::Transport,
    }
}

#[tokio::test]
#[ignore = "requires a live legacy SCS with aligned schema; set SCS_LEGACY_BASE_URL"]
async fn business_sweep_read_only_endpoints() {
    let base = std::env::var("SCS_LEGACY_BASE_URL")
        .expect("set SCS_LEGACY_BASE_URL to a live legacy SCS instance to run this");
    let adapter = ScsLegacyAdapter::with_base_url(base);

    let token = login(&adapter).await;
    println!("logged in, token acquired");

    // Read-only (resource, action) pairs from the advertised contract.
    let targets: Vec<(String, String)> = adapter
        .resources()
        .iter()
        .filter(|r| !SKIP_MODULES.contains(&r.id.as_str()))
        .flat_map(|r| {
            let rid = r.id.clone();
            r.actions
                .iter()
                .filter(|a| is_read_only(&a.method, &a.id))
                .map(move |a| (rid.clone(), a.id.clone()))
        })
        .collect();
    let total = targets.len();

    // Generous default params so list-style endpoints actually return data.
    let probe_params = json!({"page_num": 1, "page_size": 5, "page": 1, "pageSize": 5});

    let results: Vec<Class> = stream::iter(targets.clone())
        .map(|(rid, aid)| {
            let adapter = &adapter;
            let token = token.as_str();
            let params = probe_params.clone();
            async move {
                match tokio::time::timeout(
                    PROBE_TIMEOUT,
                    adapter.exec(&ctx(&rid, &aid, params, Some(token))),
                )
                .await
                {
                    Ok(result) => classify(result),
                    Err(_) => Class::Slow,
                }
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    let mut reachable = 0usize;
    let mut sys_errors: Vec<String> = Vec::new();
    let mut routed_miss = 0usize;
    let mut slow = 0usize;
    let mut transport = 0usize;
    for (i, c) in results.iter().enumerate() {
        match c {
            Class::BusinessReachable => reachable += 1,
            Class::SystemError(m) => {
                let (r, a) = &targets[i];
                sys_errors.push(format!("{r}.{a}: {m}"));
            }
            Class::RoutedMiss => routed_miss += 1,
            Class::Slow => slow += 1,
            Class::Transport => transport += 1,
        }
    }

    println!("\n=== business reachability sweep (read-only) ===");
    println!("read-only endpoints : {total}");
    println!("business-reachable  : {reachable}");
    println!("system-error        : {}", sys_errors.len());
    println!("routed-miss         : {routed_miss}");
    println!("slow (>8s)          : {slow}");
    println!("transport-error     : {transport}");
    if !sys_errors.is_empty() {
        println!("\n-- system errors (schema/dependency, first 40) --");
        for e in sys_errors.iter().take(40) {
            println!("  {e}");
        }
    }

    // The whole point of the schema alignment: business logic should run for the
    // vast majority of read-only endpoints. Allow a small tail (external deps,
    // long-tail schema, special handlers).
    let sys_rate = sys_errors.len() as f64 / total.max(1) as f64;
    assert!(
        sys_rate < 0.15,
        "system-error rate too high: {}/{} ({:.0}%) — schema not aligned?",
        sys_errors.len(),
        total,
        sys_rate * 100.0
    );
    assert_eq!(
        routed_miss, 0,
        "read-only endpoints should all route (see L2)"
    );
}
