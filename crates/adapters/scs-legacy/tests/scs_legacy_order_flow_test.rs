//! Offline end-to-end test of the SCS **order flow** through `ScsLegacyAdapter`,
//! driven by a wiremock mock server (no real legacy SCS required).
//!
//! This mirrors the orchestration in `scripts/order_flow_relais.py` /
//! `skills/scs-order-flow/SKILL.md`: goods query -> add cart -> match receiver ->
//! query cart -> select -> submit order. It pins, offline, the two facts the
//! skill verified live against production:
//!
//!   * `goods.website_goods_to` routes to `/1/goods/website_goods_to` — **no
//!     `edi_api` prefix** (the swagger spec path is the one relais uses).
//!   * `customer_id` / `receive_address_id` are sent as **strings** (numbers are
//!     rejected by legacy as `err_code 201`).
//!
//! plus the usual adapter invariants: `acs_token` injected into every body, each
//! step routed to the right `/1/<module>/<action>` path, and the flow carries an
//! order response back to the caller.
use relais_adapter_scs_legacy::ScsLegacyAdapter;
use relais_core::{Adapter, Credentials, ExecContext};
use serde_json::{json, Value};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "tok-test";
const CUSTOMER_ID: &str = "55";
const CART_ID: i64 = 218369;
const RECEIVE_ID: i64 = 93;

fn ctx(resource: &str, action: &str, params: Value) -> ExecContext {
    ExecContext {
        site: "scs".into(),
        resource: resource.into(),
        action: action.into(),
        params,
        credentials: Some(Credentials::api_key(TOKEN)),
    }
}

async fn mount_flow(server: &MockServer) {
    // 2. find goods — note the path has NO `edi_api` prefix.
    Mock::given(method("POST"))
        .and(path("/1/goods/website_goods_to"))
        .and(body_partial_json(
            json!({"acs_token": TOKEN, "service_object_id": CUSTOMER_ID, "keyword": "拉弗格10年"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data_list": [{
                "name": "拉弗格10年单一麦芽威士忌",
                "goods_id": "12633", "supplier_id": "81", "unit_id": "46",
                "rating_form_detail_id": "714927", "flat_price_id": "",
                "agent_id": "", "agent_price_id": "", "agent_cust_price_id": "",
                "supplier_alias_id": "", "goods_count_to_shopping_cart": 1
            }]
        })))
        .mount(server)
        .await;

    // 3. add to cart — acs_token injected, order_amount is an integer.
    Mock::given(method("POST"))
        .and(path("/1/shoppingcarts/create"))
        .and(body_partial_json(
            json!({"acs_token": TOKEN, "goods_id": "12633", "order_amount": 2, "is_recycle_bottle": 0}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": CART_ID})))
        .mount(server)
        .await;

    // 4. receiving address list.
    Mock::given(method("POST"))
        .and(path("/1/customers/customers_receive_list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data_list": [
                {"contact_info": "djjcvb", "receive_id": 1, "customer_name": "x"},
                {"contact_info": "测试", "receive_id": RECEIVE_ID, "customer_name": "广东20支测试门店"}
            ]
        })))
        .mount(server)
        .await;

    // 5/7. cart query — receive_address_id MUST be the string "93". The single
    // canned response satisfies both the find-target and verify-selection reads:
    // the target is present, amount already 2 (no update), and selected.
    Mock::given(method("POST"))
        .and(path("/1/shoppingcarts/getShoppingCarts"))
        .and(body_partial_json(json!({
            "acs_token": TOKEN, "customer_id": CUSTOMER_ID, "receive_address_id": "93"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_amt": 598,
            "cart_items": [{
                "cart_id": CART_ID, "goods_id": "12633", "goods_name": "拉弗格10年单一麦芽威士忌",
                "amount": 2, "select_status": 1, "supplier_id": "81",
                "goods_count_to_shopping_cart": 1
            }]
        })))
        .mount(server)
        .await;

    // 7. select.
    Mock::given(method("POST"))
        .and(path("/1/shoppingcarts/select"))
        .and(body_partial_json(
            json!({"acs_token": TOKEN, "service_object_id": CUSTOMER_ID}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(server)
        .await;

    // 8. submit order — total_amt carried as a string.
    Mock::given(method("POST"))
        .and(path("/1/orders/order_for_all"))
        .and(body_partial_json(
            json!({"acs_token": TOKEN, "total_amt": "598"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data_list": [{"order_info": [{"order_id": "192076", "order_sub_no": "0001"}]}]
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn order_flow_offline_end_to_end() {
    let server = MockServer::start().await;
    mount_flow(&server).await;
    let adapter = ScsLegacyAdapter::with_base_url(server.uri());

    // 2. find goods (spec path, no edi_api).
    let goods = adapter
        .exec(&ctx(
            "goods",
            "website_goods_to",
            json!({"service_object_id": CUSTOMER_ID, "page_num": 1, "page_size": 20, "keyword": "拉弗格10年"}),
        ))
        .await
        .expect("goods query");
    let g = &goods.data["data_list"][0];
    assert_eq!(g["goods_id"], "12633");

    // 3. add to cart.
    let create = adapter
        .exec(&ctx(
            "shoppingcarts",
            "create",
            json!({
                "service_object_id": CUSTOMER_ID, "goods_id": g["goods_id"],
                "supplier_id": g["supplier_id"], "unit_id": g["unit_id"],
                "rating_form_detail_id": g["rating_form_detail_id"],
                "flat_price_id": g["flat_price_id"], "agent_id": g["agent_id"],
                "agent_price_id": g["agent_price_id"], "agent_cust_price_id": g["agent_cust_price_id"],
                "supplier_alias_id": g["supplier_alias_id"], "order_amount": 2, "is_recycle_bottle": 0
            }),
        ))
        .await
        .expect("cart create");
    let cart_id = create.data["id"].clone();
    assert_eq!(cart_id, json!(CART_ID));

    // 4. receiving address — exact contact_info match.
    let addr = adapter
        .exec(&ctx(
            "customers",
            "customers_receive_list",
            json!({"customer_id": CUSTOMER_ID, "page_num": 1, "page_size": 50, "receive_type": 0}),
        ))
        .await
        .expect("address list");
    let receive_id = addr.data["data_list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["contact_info"] == "测试")
        .map(|a| a["receive_id"].clone())
        .expect("receiver 测试");
    assert_eq!(receive_id, json!(RECEIVE_ID));

    // 5. cart query — receive_address_id as a string.
    let receive_id_str = receive_id.as_i64().unwrap().to_string();
    let cart = adapter
        .exec(&ctx(
            "shoppingcarts",
            "getShoppingCarts",
            json!({"customer_id": CUSTOMER_ID, "is_recycle_bottle": 0, "receive_address_id": receive_id_str}),
        ))
        .await
        .expect("cart query");
    let items = cart.data["cart_items"].as_array().unwrap();
    let target = items
        .iter()
        .find(|i| i["cart_id"] == cart_id)
        .expect("target cart item");
    assert_eq!(target["amount"], 2);

    // 7. select only the target, then verify exactly one selected.
    let select_list: Vec<Value> = items
        .iter()
        .map(|i| json!({"id": i["cart_id"], "select_status": if i["cart_id"] == cart_id {1} else {2}}))
        .collect();
    adapter
        .exec(&ctx(
            "shoppingcarts",
            "select",
            json!({"service_object_id": CUSTOMER_ID, "select": select_list}),
        ))
        .await
        .expect("select");
    let cart = adapter
        .exec(&ctx(
            "shoppingcarts",
            "getShoppingCarts",
            json!({"customer_id": CUSTOMER_ID, "is_recycle_bottle": 0, "receive_address_id": receive_id_str}),
        ))
        .await
        .expect("cart re-query");
    let selected: Vec<&Value> = cart.data["cart_items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["select_status"] == 1)
        .collect();
    assert_eq!(selected.len(), 1, "exactly one selected before order");
    assert_eq!(selected[0]["cart_id"], cart_id);

    // 8. submit order — total_amt carried as a string.
    let total_amt = cart.data["total_amt"].as_i64().unwrap().to_string();
    let order = adapter
        .exec(&ctx(
            "orders",
            "order_for_all",
            json!({
                "order_no": "260614101412170000",
                "address_item": [{"agent_id": "", "customer_id": CUSTOMER_ID, "receive_info_id": receive_id}],
                "shopping_cart_ids": [cart_id],
                "total_amt": total_amt,
                "shopping_carts": [{"id": cart_id, "activity_customer_type": 0, "activity_ids": []}],
                "service_object_id": CUSTOMER_ID, "recycle_bottle_voucher": []
            }),
        ))
        .await
        .expect("order submit");
    assert_eq!(
        order.data["data_list"][0]["order_info"][0]["order_id"],
        "192076"
    );
}
