use std::collections::HashMap;

use relais_core::{AuthType, CredentialData, Credentials};

#[test]
fn cookie_credentials_can_be_extracted() {
    let mut cookies = HashMap::new();
    cookies.insert("session".into(), "abc123".into());
    cookies.insert("csrf".into(), "xyz789".into());

    let cred = Credentials {
        credential_type: AuthType::Cookie,
        data: CredentialData::Cookie {
            cookies,
            domain: "example.com".into(),
            captured_at: chrono::Utc::now(),
            expires_at: None,
        },
    };

    match &cred.data {
        CredentialData::Cookie { cookies, domain, .. } => {
            assert_eq!(cookies.get("session"), Some(&"abc123".to_string()));
            assert_eq!(cookies.get("csrf"), Some(&"xyz789".to_string()));
            assert_eq!(domain, "example.com");
        }
        _ => panic!("expected cookie credentials"),
    }
}

#[test]
fn cookie_credentials_have_no_bearer_token() {
    let mut cookies = HashMap::new();
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
fn api_key_credentials_have_no_cookies() {
    let cred = Credentials::api_key("ghp_test123");

    match &cred.data {
        CredentialData::Cookie { .. } => panic!("expected non-cookie credentials"),
        _ => {} // Good: API key is not a cookie
    }
}
