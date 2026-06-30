//! Machine-readable error envelope for the ingress HTTP API.
//!
//! Every error response the API returns has the shape:
//!
//! ```json
//! { "error": { "code": "TRANSITION_MISMATCH", "message": "..." } }
//! ```
//!
//! The `code` is a stable, uppercase snake-case identifier that integrators match on
//! programmatically; the `message` is a human-readable explanation that may change at
//! any time. Status codes are carried alongside the code internally and are unchanged
//! from the per-endpoint behaviour they replace.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, async_trait};
use serde::{Deserialize, Serialize};

/// Stable, machine-readable error code returned in every API error response.
///
/// These identifiers are a public API contract: integrators branch on them, so a variant
/// must never be renamed or repurposed once shipped. Add a new variant rather than changing
/// the meaning of an existing one. The wire form is uppercase snake-case (e.g.
/// `TRANSITION_MISMATCH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// A required address field is zero or otherwise not a usable address.
    InvalidAddress,
    /// `block_height` is older than the accepted staleness window.
    StaleBlock,
    /// The provided `transition_index` does not match the contract's current state.
    TransitionMismatch,
    /// `call_data` exceeds the maximum accepted size.
    CalldataTooLarge,
    /// The client exceeded its allotted request rate.
    RateLimited,
    /// The ingress queue is at capacity and cannot accept more work right now.
    QueueFull,
    /// The request is missing valid authentication credentials.
    Unauthorized,
    /// An upstream RPC endpoint failed or is unreachable.
    RpcUnavailable,
    /// No contract is deployed at the requested target address on any supported chain.
    ContractNotFound,
    /// The request body is malformed or fails field-level validation.
    InvalidRequest,
    /// The requested path does not exist.
    NotFound,
    /// The HTTP method is not supported for the requested path.
    MethodNotAllowed,
    /// An unexpected server-side error occurred.
    Internal,
}

/// The `error` object nested inside [`ApiErrorEnvelope`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub code: ErrorCode,
    pub message: String,
}

/// The full wire shape of an error response: `{ "error": { "code", "message" } }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorEnvelope {
    pub error: ApiErrorBody,
}

/// An API error in flight: the HTTP status to return plus the [`ErrorCode`] and message
/// that are serialized into the [`ApiErrorEnvelope`] body.
///
/// Implements [`IntoResponse`], so handlers return `Result<T, ApiError>` and `?`/`Err`
/// produce a correctly-shaped response with the carried status.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: ErrorCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// 401 with [`ErrorCode::Unauthorized`].
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "Unauthorized",
        )
    }

    /// 503 with [`ErrorCode::QueueFull`].
    pub fn queue_full(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::QueueFull,
            message,
        )
    }

    /// 500 with [`ErrorCode::Internal`].
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            message,
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorEnvelope {
            error: ApiErrorBody {
                code: self.code,
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

/// A `Json<T>` extractor that emits the [`ApiErrorEnvelope`] on failure instead of axum's
/// default plain-text body, so body-parse and schema-validation failures share the same
/// error contract as handler-level errors. The status code from the underlying
/// [`JsonRejection`] is preserved (400 for malformed JSON, 422 for shape mismatch, etc.).
pub struct ApiJson<T>(pub T);

#[async_trait]
impl<S, T> FromRequest<S> for ApiJson<T>
where
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(ApiError::new(
                rejection.status(),
                ErrorCode::InvalidRequest,
                rejection.body_text(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[test]
    fn error_codes_serialize_as_screaming_snake_case() {
        let cases = [
            (ErrorCode::InvalidAddress, "INVALID_ADDRESS"),
            (ErrorCode::StaleBlock, "STALE_BLOCK"),
            (ErrorCode::TransitionMismatch, "TRANSITION_MISMATCH"),
            (ErrorCode::CalldataTooLarge, "CALLDATA_TOO_LARGE"),
            (ErrorCode::RateLimited, "RATE_LIMITED"),
            (ErrorCode::QueueFull, "QUEUE_FULL"),
            (ErrorCode::Unauthorized, "UNAUTHORIZED"),
            (ErrorCode::RpcUnavailable, "RPC_UNAVAILABLE"),
            (ErrorCode::ContractNotFound, "CONTRACT_NOT_FOUND"),
            (ErrorCode::InvalidRequest, "INVALID_REQUEST"),
            (ErrorCode::NotFound, "NOT_FOUND"),
            (ErrorCode::MethodNotAllowed, "METHOD_NOT_ALLOWED"),
            (ErrorCode::Internal, "INTERNAL"),
        ];
        for (code, wire) in cases {
            assert_eq!(serde_json::to_value(code).unwrap(), serde_json::json!(wire));
        }
    }

    #[tokio::test]
    async fn into_response_carries_status_and_envelope() {
        let err = ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::TransitionMismatch,
            "expected transition_index 42, contract reports 43",
        );
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: ApiErrorEnvelope = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.error.code, ErrorCode::TransitionMismatch);
        assert_eq!(
            parsed.error.message,
            "expected transition_index 42, contract reports 43"
        );
    }

    #[test]
    fn envelope_has_no_extra_top_level_fields() {
        let json = serde_json::to_value(ApiErrorEnvelope {
            error: ApiErrorBody {
                code: ErrorCode::QueueFull,
                message: "full".to_string(),
            },
        })
        .unwrap();
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        assert!(obj.contains_key("error"));
    }
}
