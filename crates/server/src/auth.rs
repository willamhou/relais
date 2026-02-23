use axum::{
    extract::Request,
    http::{self, StatusCode},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}

/// Middleware that validates JWT tokens on incoming requests.
///
/// Extracts the token from the "Authorization: Bearer <token>" header,
/// decodes it with HS256, and returns 401 if missing or invalid.
pub async fn require_jwt(
    state: axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth_header = request
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(v) if v.starts_with("Bearer ") => &v[7..],
        _ => return Err(StatusCode::UNAUTHORIZED),
    };

    let validation = Validation::default();
    let key = DecodingKey::from_secret(state.jwt_secret.as_bytes());

    match decode::<Claims>(token, &key, &validation) {
        Ok(_) => Ok(next.run(request).await),
        Err(_) => Err(StatusCode::UNAUTHORIZED),
    }
}
