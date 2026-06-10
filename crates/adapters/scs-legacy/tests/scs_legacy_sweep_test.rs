//! L2 — contract conformance sweep.
//!
//! Probes EVERY legacy endpoint against a live legacy SCS and asserts the
//! adapter's generated `(method, path)` actually routes — i.e. legacy does NOT
//! answer with its "route not found" error (`{"err_code":"201","err_msg":"请求的服务不存在"}`).
//! This verifies all 1324 path mappings at once, without depending on login or DB
//! state: an unauthenticated/invalid-token request still reaches the route (and
//! fails at the auth or business layer), which is exactly what we assert.
//!
//! Ignored by default; needs a live legacy at `SCS_LEGACY_BASE_URL`:
//!
//! ```sh
//! SCS_LEGACY_BASE_URL=http://127.0.0.1:8501 \
//!   cargo test -p relais-adapter-scs-legacy --test scs_legacy_sweep_test -- --ignored --nocapture
//! ```
//!
//! SAFETY: probes carry an invalid `acs_token`, so token-auth endpoints (the vast
//! majority) fail at the auth layer with no side effects. A handful of none-auth
//! modules (demos/sysadmin/xbb/yunst/tests/unite) DO execute — run this only
//! against a disposable test database.
use futures::stream::{self, StreamExt};
use relais_adapter_scs_legacy::ScsLegacyAdapter;
use relais_core::{Adapter, AdapterError, Credentials, ExecContext};
use serde_json::json;

/// legacy's global "route not found" message (controllers/global_error__controller.go).
const ROUTE_NOT_FOUND: &str = "请求的服务不存在";
const CONCURRENCY: usize = 16;

#[derive(Debug)]
enum Probe {
    /// Route reached (auth/business response, or success) — what we want.
    Hit,
    /// legacy returned its "route not found" error, or HTTP 404 — mapping mismatch.
    Miss(String),
    /// Transport/other adapter error (environment, not a mapping result).
    Err(String),
}

#[tokio::test]
#[ignore = "requires a live legacy SCS; set SCS_LEGACY_BASE_URL and run with --ignored"]
async fn sweep_all_endpoints_route_on_live_legacy() {
    let base = std::env::var("SCS_LEGACY_BASE_URL")
        .expect("set SCS_LEGACY_BASE_URL to a live legacy SCS instance to run this");
    let adapter = ScsLegacyAdapter::with_base_url(base);

    // Every (resource, action) the adapter advertises — the full 1324.
    let probes: Vec<(String, String)> = adapter
        .resources()
        .iter()
        .flat_map(|r| {
            let rid = r.id.clone();
            r.actions.iter().map(move |a| (rid.clone(), a.id.clone()))
        })
        .collect();
    let total = probes.len();

    let results: Vec<(String, String, Probe)> = stream::iter(probes)
        .map(|(rid, aid)| {
            let adapter = &adapter;
            async move {
                let ctx = ExecContext {
                    site: "scs".into(),
                    resource: rid.clone(),
                    action: aid.clone(),
                    params: json!({}),
                    // invalid token -> token-auth routes fail at the auth layer (no side effects)
                    credentials: Some(Credentials::api_key("l2-sweep-invalid-probe")),
                };
                let probe = match adapter.exec(&ctx).await {
                    Ok(resp) => {
                        let msg = resp
                            .data
                            .get("err_msg")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if msg == ROUTE_NOT_FOUND {
                            Probe::Miss("route-not-found".into())
                        } else {
                            Probe::Hit
                        }
                    }
                    Err(AdapterError::NotFound(_)) => Probe::Miss("http-404".into()),
                    Err(e) => Probe::Err(e.to_string()),
                };
                (rid, aid, probe)
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    let mut hits = 0usize;
    let mut misses: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (rid, aid, probe) in &results {
        match probe {
            Probe::Hit => hits += 1,
            Probe::Miss(why) => misses.push(format!("{rid}.{aid} ({why})")),
            Probe::Err(e) => errors.push(format!("{rid}.{aid}: {e}")),
        }
    }

    println!("\n=== L2 contract sweep ===");
    println!("total endpoints : {total}");
    println!("routed (hit)    : {hits}");
    println!("not routed (miss): {}", misses.len());
    println!("transport errors: {}", errors.len());
    if !misses.is_empty() {
        println!("\n-- misses (adapter path not found on live legacy) --");
        for m in misses.iter().take(50) {
            println!("  {m}");
        }
        if misses.len() > 50 {
            println!("  ... and {} more", misses.len() - 50);
        }
    }
    if !errors.is_empty() {
        println!("\n-- transport errors (environment, first 20) --");
        for e in errors.iter().take(20) {
            println!("  {e}");
        }
    }

    // Transport errors mean the legacy instance is unreachable/unstable — fail loudly.
    assert!(
        errors.len() < total / 10,
        "too many transport errors ({}/{}); is legacy healthy?",
        errors.len(),
        total
    );
    // Every advertised endpoint must route on real legacy.
    assert!(
        misses.is_empty(),
        "{} of {} endpoints did not route on live legacy (see list above)",
        misses.len(),
        total
    );
}
