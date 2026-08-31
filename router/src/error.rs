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

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, async_trait};
use serde::{Deserialize, Serialize};

/// Stable, machine-readable error code returned in every API error response.
///
/// These identifiers are a public API contract: integrators branch on them, so a variant
/// must never be renamed or repurposed once shipped. Add a new variant rather than changing
/// the meaning of an existing one. The wire form is uppercase snake-case (e.g.
/// `TRANSITION_MISMATCH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// A required address field is zero or otherwise not a usable address.
    InvalidAddress,
    /// `block_height` is older than the accepted staleness window.
    StaleBlock,
    /// The provided `transition_index` does not match the contract's current state.
    TransitionMismatch,
    /// A previously rendered `ready` payload is no longer submittable — its `valid_until_block`
    /// has passed or the on-chain transition index has advanced — so the caller must re-request.
    PayloadExpired,
    /// `call_data` exceeds the maximum accepted size.
    CalldataTooLarge,
    /// The client exceeded its allotted request rate.
    RateLimited,
    /// The ingress queue is at capacity and cannot accept more work right now.
    QueueFull,
    /// The request is missing valid authentication credentials.
    Unauthorized,
    /// The caller is authenticated but not permitted to access the requested resource (e.g. a
    /// task owned by a different API key).
    Forbidden,
    /// An upstream RPC endpoint failed or is unreachable.
    RpcUnavailable,
    /// No contract is deployed at the requested target address on any supported chain.
    ContractNotFound,
    /// The target address holds code now but held none at the requested `block_height` — the
    /// request is anchored before the target was deployed.
    TargetNotDeployed,
    /// The request body is malformed or fails field-level validation.
    InvalidRequest,
    /// The requested path does not exist.
    NotFound,
    /// The HTTP method is not supported for the requested path.
    MethodNotAllowed,
    /// A required server-side feature or credential is not configured, so the endpoint cannot
    /// serve the request (e.g. the admin API without `ADMIN_KEY`, or persistence disabled).
    NotConfigured,
    /// An unexpected server-side error occurred.
    Internal,
}

/// The `error` object nested inside [`ApiErrorEnvelope`].
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApiErrorBody {
    pub code: ErrorCode,
    pub message: String,
}

/// The full wire shape of an error response: `{ "error": { "code", "message" } }`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// When set, emitted as a `Retry-After` header (in seconds) so a client that hit a
    /// transient limit knows roughly how long to wait before retrying. Only meaningful for
    /// retryable statuses (e.g. 503 `QUEUE_FULL`).
    pub retry_after_secs: Option<u64>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after_secs: None,
        }
    }

    /// Attaches a `Retry-After` estimate (in seconds), returned as a header on the response.
    pub fn with_retry_after(mut self, secs: u64) -> Self {
        self.retry_after_secs = Some(secs);
        self
    }

    /// 401 with [`ErrorCode::Unauthorized`].
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "Unauthorized",
        )
    }

    /// 403 with [`ErrorCode::Forbidden`].
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, ErrorCode::Forbidden, message)
    }

    /// 404 with [`ErrorCode::NotFound`].
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, message)
    }

    /// 409 with [`ErrorCode::PayloadExpired`]. Signals that a `ready` payload has gone stale and
    /// the caller must re-request; 409 (not 410) because the task still exists and the resolution
    /// is to submit a fresh request, not that the resource is permanently gone.
    pub fn payload_expired(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, ErrorCode::PayloadExpired, message)
    }

    /// 429 with [`ErrorCode::RateLimited`], carrying the `Retry-After` delay (seconds) until the
    /// caller's next request is allowed under its per-key rate limit.
    pub fn rate_limited(retry_after_secs: u64) -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::RateLimited,
            "Rate limit exceeded; retry after the indicated delay",
        )
        .with_retry_after(retry_after_secs)
    }

    /// 503 with [`ErrorCode::QueueFull`], carrying a `Retry-After` estimate (seconds) so a
    /// load-shed client backs off rather than hot-looping against a full queue.
    pub fn queue_full(message: impl Into<String>, retry_after_secs: u64) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::QueueFull,
            message,
        )
        .with_retry_after(retry_after_secs)
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
        let retry_after = self.retry_after_secs;
        let status = self.status;
        let body = ApiErrorEnvelope {
            error: ApiErrorBody {
                code: self.code,
                message: self.message,
            },
        };
        let mut response = (status, Json(body)).into_response();
        if let Some(secs) = retry_after
            && let Ok(value) = HeaderValue::from_str(&secs.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
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

/// A `Query<T>` extractor that emits the [`ApiErrorEnvelope`] on failure instead of axum's
/// default plain-text body, so a malformed query string (an unknown `?status=` value, a
/// non-integer `?limit=`) shares the same error contract as the rest of the API. The status
/// from the underlying [`QueryRejection`] is preserved (400).
pub struct ApiQuery<T>(pub T);

#[async_trait]
impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    Query<T>: FromRequestParts<S, Rejection = QueryRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(ApiQuery(value)),
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
            (ErrorCode::PayloadExpired, "PAYLOAD_EXPIRED"),
            (ErrorCode::CalldataTooLarge, "CALLDATA_TOO_LARGE"),
            (ErrorCode::RateLimited, "RATE_LIMITED"),
            (ErrorCode::QueueFull, "QUEUE_FULL"),
            (ErrorCode::Unauthorized, "UNAUTHORIZED"),
            (ErrorCode::Forbidden, "FORBIDDEN"),
            (ErrorCode::RpcUnavailable, "RPC_UNAVAILABLE"),
            (ErrorCode::ContractNotFound, "CONTRACT_NOT_FOUND"),
            (ErrorCode::TargetNotDeployed, "TARGET_NOT_DEPLOYED"),
            (ErrorCode::InvalidRequest, "INVALID_REQUEST"),
            (ErrorCode::NotFound, "NOT_FOUND"),
            (ErrorCode::MethodNotAllowed, "METHOD_NOT_ALLOWED"),
            (ErrorCode::NotConfigured, "NOT_CONFIGURED"),
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
    fn payload_expired_carries_conflict_status() {
        let err = ApiError::payload_expired("valid_until_block 100 passed; re-request");
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.code, ErrorCode::PayloadExpired);
    }

    #[tokio::test]
    async fn queue_full_response_sets_retry_after_header() {
        let resp = ApiError::queue_full("full", 60).into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            resp.headers().get(header::RETRY_AFTER).unwrap(),
            &HeaderValue::from_static("60")
        );
    }

    #[tokio::test]
    async fn error_without_retry_after_omits_the_header() {
        let resp = ApiError::not_found("nope").into_response();
        assert!(resp.headers().get(header::RETRY_AFTER).is_none());
    }

    #[tokio::test]
    async fn rate_limited_response_sets_429_and_retry_after() {
        let resp = ApiError::rate_limited(42).into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers().get(header::RETRY_AFTER).unwrap(),
            &HeaderValue::from_static("42")
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
