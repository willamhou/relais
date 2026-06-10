//! End-to-end test against a *live* SCS instance.
//!
//! Ignored by default (it needs a running SCS account service + database).
//! To run it, start SCS and point the adapter at it:
//!
//! ```sh
//! # in the scs repo:
//! docker compose -f deploy/docker-compose.yaml up -d postgres redis
//! # run the account service (host go, or a golang container with --network host)
//!
//! # then, in the relais repo:
//! SCS_BASE_URL=http://127.0.0.1:8000 \
//!   cargo test -p relais-adapter-scs --test scs_e2e_test -- --ignored
//! ```
use relais_adapter_scs::ScsAdapter;
use relais_core::{Adapter, AdapterError, ExecContext};
use serde_json::{json, Value};

fn live_adapter() -> ScsAdapter {
    let base =
        std::env::var("SCS_BASE_URL").expect("set SCS_BASE_URL to a live SCS instance to run this");
    ScsAdapter::with_base_url(base)
}

fn ctx(resource: &str, action: &str, params: Value) -> ExecContext {
    ExecContext {
        site: "scs".into(),
        resource: resource.into(),
        action: action.into(),
        params,
        credentials: None,
    }
}

#[tokio::test]
#[ignore = "requires a live SCS instance; set SCS_BASE_URL and run with --ignored"]
async fn e2e_account_crud_roundtrip() {
    let a = live_adapter();

    // create
    let created = a
        .exec(&ctx(
            "accounts",
            "create",
            json!({"name": "E2E", "phone": "12345", "type": 2}),
        ))
        .await
        .expect("create should succeed");
    assert_eq!(created.data["name"], "E2E");
    // protobuf JSON serializes int64 id as a string.
    let id: i64 = created.data["id"]
        .as_str()
        .expect("id should be a string")
        .parse()
        .expect("id should parse");
    assert!(id > 0);

    // get
    let got = a
        .exec(&ctx("accounts", "get", json!({ "id": id })))
        .await
        .expect("get should succeed");
    assert_eq!(got.data["id"], created.data["id"]);
    assert_eq!(got.data["name"], "E2E");

    // list fills offset pagination metadata
    let listed = a
        .exec(&ctx(
            "accounts",
            "list",
            json!({"page": 1, "page_size": 50}),
        ))
        .await
        .expect("list should succeed");
    let pg = listed
        .meta
        .pagination
        .expect("list should populate pagination");
    assert!(pg.total.is_some(), "total should be parsed from the reply");

    // update (full replace)
    let updated = a
        .exec(&ctx(
            "accounts",
            "update",
            json!({"id": id, "name": "E2E-Updated", "phone": "67890", "type": 2}),
        ))
        .await
        .expect("update should succeed");
    assert_eq!(updated.data["name"], "E2E-Updated");
    assert_eq!(updated.data["phone"], "67890");

    // delete returns {success: true}
    let deleted = a
        .exec(&ctx("accounts", "delete", json!({ "id": id })))
        .await
        .expect("delete should succeed");
    assert_eq!(deleted.data["success"], true);

    // get after delete -> NotFound
    let err = a
        .exec(&ctx("accounts", "get", json!({ "id": id })))
        .await
        .expect_err("get on a deleted account should fail");
    assert!(matches!(err, AdapterError::NotFound(_)), "got {err:?}");
}

#[tokio::test]
#[ignore = "requires a live SCS instance; set SCS_BASE_URL and run with --ignored"]
async fn e2e_get_missing_is_not_found() {
    let a = live_adapter();
    let err = a
        .exec(&ctx("accounts", "get", json!({"id": 999_999_999})))
        .await
        .expect_err("a non-existent account should not be found");
    assert!(matches!(err, AdapterError::NotFound(_)), "got {err:?}");
}
