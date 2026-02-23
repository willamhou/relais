use relais_core::{
    Action, AuthType, CredentialData, Credentials, Method, PaginationInfo, PaginationStyle,
    RateLimit, Resource, Response, ResponseMeta, SiteManifest,
};
use serde_json::json;

#[test]
fn site_manifest_serializes() {
    let manifest = SiteManifest {
        id: "github".into(),
        name: "GitHub".into(),
        base_url: "https://github.com".into(),
        auth_type: AuthType::APIKey,
    };
    let json = serde_json::to_value(&manifest).unwrap();
    assert_eq!(json["id"], "github");
    assert_eq!(json["auth_type"], "APIKey");
}

#[test]
fn resource_tree_navigates() {
    let resource = Resource {
        id: "repos".into(),
        description: "GitHub repositories".into(),
        actions: vec![Action {
            id: "list".into(),
            method: Method::Read,
            description: "List repositories".into(),
            params: json!({"type": "object"}),
            returns: json!({"type": "array"}),
            pagination: Some(PaginationStyle::Cursor),
        }],
        children: vec![Resource {
            id: "issues".into(),
            description: "Repository issues".into(),
            actions: vec![],
            children: vec![],
        }],
    };
    assert_eq!(resource.children[0].id, "issues");
    assert!(resource.actions[0].pagination.is_some());
}

#[test]
fn response_meta_includes_pagination() {
    let response = Response {
        data: json!({"items": []}),
        meta: ResponseMeta {
            pagination: Some(PaginationInfo {
                has_next: true,
                cursor: Some("abc123".into()),
                total: Some(42),
            }),
            rate_limit: Some(RateLimit {
                remaining: 58,
                reset_at: chrono::Utc::now(),
            }),
            cached: false,
        },
    };
    assert!(response.meta.pagination.as_ref().unwrap().has_next);
    assert_eq!(response.meta.rate_limit.as_ref().unwrap().remaining, 58);
}

#[test]
fn credentials_api_key_serializes() {
    let cred = Credentials::api_key("ghp_test123");
    let json = serde_json::to_string(&cred).unwrap();
    let deserialized: Credentials = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.bearer_token(), Some("ghp_test123"));
}

#[test]
fn credentials_oauth_serializes() {
    let cred = Credentials::oauth("access_abc", Some("refresh_xyz".into()), None);
    let json = serde_json::to_string(&cred).unwrap();
    let deserialized: Credentials = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.bearer_token(), Some("access_abc"));
    assert!(!deserialized.is_expired());
}

#[test]
fn credentials_oauth_expired() {
    use chrono::{Duration, Utc};
    let past = Utc::now() - Duration::hours(1);
    let cred = Credentials::oauth("old_token", None, Some(past));
    assert!(cred.is_expired());
}

#[test]
fn credentials_cookie_has_no_bearer() {
    let mut cookies = std::collections::HashMap::new();
    cookies.insert("session".into(), "abc123".into());
    let cred = Credentials {
        credential_type: AuthType::Cookie,
        data: CredentialData::Cookie {
            cookies,
            domain: "example.com".into(),
            captured_at: chrono::Utc::now(),
            expires_at: None,
        },
    };
    assert_eq!(cred.bearer_token(), None);
}

#[test]
fn cookie_not_stale_when_fresh() {
    let mut cookies = std::collections::HashMap::new();
    cookies.insert("session".into(), "abc".into());
    let cred = Credentials {
        credential_type: AuthType::Cookie,
        data: CredentialData::Cookie {
            cookies,
            domain: "example.com".into(),
            captured_at: chrono::Utc::now(),
            expires_at: None,
        },
    };
    assert!(!cred.is_cookie_stale(24));
}

#[test]
fn cookie_stale_when_old() {
    use chrono::{Duration, Utc};

    let mut cookies = std::collections::HashMap::new();
    cookies.insert("session".into(), "abc".into());
    let cred = Credentials {
        credential_type: AuthType::Cookie,
        data: CredentialData::Cookie {
            cookies,
            domain: "example.com".into(),
            captured_at: Utc::now() - Duration::hours(25),
            expires_at: None,
        },
    };
    assert!(cred.is_cookie_stale(24));
}

#[test]
fn cookie_stale_when_expired() {
    use chrono::{Duration, Utc};

    let mut cookies = std::collections::HashMap::new();
    cookies.insert("session".into(), "abc".into());
    let cred = Credentials {
        credential_type: AuthType::Cookie,
        data: CredentialData::Cookie {
            cookies,
            domain: "example.com".into(),
            captured_at: Utc::now(),
            expires_at: Some(Utc::now() - Duration::hours(1)),
        },
    };
    assert!(cred.is_cookie_stale(24));
}

#[test]
fn api_key_is_never_stale() {
    let cred = Credentials::api_key("token");
    assert!(!cred.is_cookie_stale(24));
}

#[test]
fn oauth_is_never_cookie_stale() {
    let cred = Credentials::oauth("access_abc", None, None);
    assert!(!cred.is_cookie_stale(24));
}

#[test]
fn cookie_not_stale_when_within_max_age() {
    use chrono::{Duration, Utc};

    let mut cookies = std::collections::HashMap::new();
    cookies.insert("session".into(), "abc".into());
    let cred = Credentials {
        credential_type: AuthType::Cookie,
        data: CredentialData::Cookie {
            cookies,
            domain: "example.com".into(),
            captured_at: Utc::now() - Duration::hours(23),
            expires_at: None,
        },
    };
    assert!(!cred.is_cookie_stale(24));
}

#[test]
fn cookie_not_stale_when_explicit_expiry_in_future() {
    use chrono::{Duration, Utc};

    let mut cookies = std::collections::HashMap::new();
    cookies.insert("session".into(), "abc".into());
    // Captured long ago but explicit expiry is in the future and capture age < max_age
    let cred = Credentials {
        credential_type: AuthType::Cookie,
        data: CredentialData::Cookie {
            cookies,
            domain: "example.com".into(),
            captured_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::hours(48)),
        },
    };
    assert!(!cred.is_cookie_stale(24));
}
