//! The OpenAPI document for the ingress HTTP API, generated from the handlers themselves.
//!
//! [`ApiDoc`] is the whole contract: every route [`build_app`](crate::ingress::build_app) serves
//! is listed here, and each one is described by a `#[utoipa::path]` attribute sitting on the
//! handler that implements it. Keeping the description next to the handler is the point of
//! generating rather than hand-writing the document, since a route cannot change shape without
//! the annotation being right there in the diff.
//!
//! Two schemas cannot be inferred from their Rust types and are built by hand below:
//! [`CallData`], whose serde representation is a JSON array of byte integers rather than a hex
//! string, and [`TransitionIndex`], which accepts three different JSON types. The Ethereum
//! primitives have the same problem and live in [`gas_killer_common::openapi`].
//!
//! `router/openapi.json` is this document rendered to disk. [`render`] produces it, the `openapi`
//! binary writes it, and a test below fails when the committed copy has fallen behind the code.

use crate::error::{ApiErrorBody, ApiErrorEnvelope, ErrorCode};
use crate::ingress::{
    AvsMetadata, AvsOperatorSetMetadata, AvsOperatorSetSoftware, CreateApiKeyRequest,
    GasKillerTaskRequest, GasKillerTaskRequestBody, TaskAcceptedResponse, TaskView,
};
use crate::store::{ApiKeyMetadata, CreatedApiKey, TaskStatus};
use gas_killer_common::avs_contracts::AvsContracts;
use gas_killer_common::openapi::{Address, HexBytes, HexUint256};
use gas_killer_common::payload::PayloadView;
use gas_killer_common::task_data::MAX_EVM_TX_CALLDATA_SIZE;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{
    AnyOfBuilder, ArrayBuilder, KnownFormat, ObjectBuilder, Schema, SchemaFormat, SchemaType, Type,
};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, PartialSchema, ToSchema};

/// Version of the HTTP API contract, reported as `info.version`.
///
/// Tracks the wire contract rather than the crate version: bump it when a route, a field, or an
/// error code changes shape, not on every release.
pub const API_VERSION: &str = "0.3.0";

/// Shortest accepted `call_data`, the four bytes of an ABI function selector.
const SELECTOR_LEN: usize = 4;

/// ABI-encoded calldata, serialized as a JSON array of byte values rather than a hex string.
///
/// The unusual representation is serde's default for `Vec<u8>` and is part of the published
/// contract, so it is described as it is rather than corrected here.
pub struct CallData;

impl PartialSchema for CallData {
    fn schema() -> RefOr<Schema> {
        ArrayBuilder::new()
            .items(
                ObjectBuilder::new()
                    .schema_type(SchemaType::new(Type::Integer))
                    .minimum(Some(0))
                    .maximum(Some(255)),
            )
            .min_items(Some(SELECTOR_LEN))
            .max_items(Some(MAX_EVM_TX_CALLDATA_SIZE))
            .description(Some(
                "ABI-encoded calldata as an array of byte values (0 to 255). The first four \
                 bytes are the function selector, so at least 4 bytes are required; the maximum \
                 accepted size is 128 KiB.",
            ))
            .examples([serde_json::json!([171, 205, 239, 1])])
            .into()
    }
}

impl ToSchema for CallData {}

/// The state-transition slot a submission occupies: an integer to claim a specific slot, or the
/// string `"auto"` (equivalently `null`, or the field omitted) to let the router resolve the next
/// free slot when the task is dequeued.
///
/// A three-way union, because `"auto"` has to be distinguishable from slot 0 while an absent
/// field has to keep meaning the same thing it did before the string form existed.
pub struct TransitionIndex;

impl PartialSchema for TransitionIndex {
    fn schema() -> RefOr<Schema> {
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(SchemaType::new(Type::Integer))
                    .format(Some(SchemaFormat::KnownFormat(KnownFormat::Int64)))
                    .minimum(Some(0)),
            )
            .item(
                ObjectBuilder::new()
                    .schema_type(SchemaType::new(Type::String))
                    .enum_values(Some(["auto"])),
            )
            .item(ObjectBuilder::new().schema_type(SchemaType::new(Type::Null)))
            .description(Some(
                "The state-transition slot to occupy. Send an integer to target a specific slot, \
                 or \"auto\" (or null, or omit the field) to let the router resolve the next \
                 available slot at dequeue time, which is what makes safe parallel submissions \
                 possible.",
            ))
            .default(Some(serde_json::json!("auto")))
            .into()
    }
}

impl ToSchema for TransitionIndex {}

/// Registers the two bearer schemes the routes reference.
///
/// Both are `Authorization: Bearer <token>`, but they are separate schemes because they are
/// separate credentials with different blast radii: an API key scopes a caller to its own tasks,
/// while `ADMIN_KEY` mints and revokes those keys.
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "ApiKeyAuth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "An API key for task submission and polling, sent as \
                         `Authorization: Bearer gk_...`.",
                    ))
                    .build(),
            ),
        );
        components.add_security_scheme(
            "AdminAuth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "The operator's `ADMIN_KEY` shared secret, sent as \
                         `Authorization: Bearer <ADMIN_KEY>`. Mints and revokes API keys, so it \
                         is not an integrator credential.",
                    ))
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Gas Killer Router API",
        version = API_VERSION,
        description = "HTTP API for the Gas Killer router (the AVS aggregator).

Clients submit compute tasks to `POST /tasks`; the router validates them, fans them out to \
restaked operator nodes, and aggregates the BLS signatures into a completed round. Rather than \
broadcasting on-chain itself, the router returns a ready-to-sign transaction, the *payload*. The \
typical flow is:

1. `POST /tasks` to submit a task and receive a `task_id`.
2. `GET /tasks/{task_id}` to poll until `status` is `ready`, at which point the response carries \
a `payload` object.
3. Sign and submit that `payload` from your own wallet before its `valid_until_block`.

Task submission and polling are authenticated with an API key.",
        contact(
            name = "Gas Killer",
            url = "https://gaskiller.xyz",
            email = "contact@gaskiller.xyz"
        ),
        license(name = "PPL")
    ),
    servers(
        (
            url = "{baseUrl}",
            description = "Gas Killer router ingress",
            variables(
                ("baseUrl" = (
                    default = "https://testnet.gaskiller.xyz",
                    enum_values("https://testnet.gaskiller.xyz", "http://localhost:8080"),
                    description = "Base URL of the router ingress. Defaults to the public \
                                   testnet; select localhost for local development, or replace \
                                   with your own deployment."
                ))
            )
        )
    ),
    tags(
        (
            name = "Tasks",
            description = "Submitting compute tasks, polling their status, and retrieving the \
                           ready-to-sign payload."
        ),
        (
            name = "Metadata",
            description = "Public AVS identity and settlement addresses, consumed by the \
                           restaking indexer and by integrators wiring a target contract."
        ),
        (
            name = "Admin",
            description = "API key lifecycle, guarded by the operator's `ADMIN_KEY`. Operator \
                           surface, not part of the integrator API."
        ),
        (
            name = "Health",
            description = "Liveness probing."
        )
    ),
    paths(
        crate::ingress::submit_task_handler,
        crate::ingress::list_tasks_handler,
        crate::ingress::get_task_handler,
        crate::ingress::avs_metadata_handler,
        crate::ingress::healthz_handler,
        crate::ingress::create_api_key_handler,
        crate::ingress::list_api_keys_handler,
        crate::ingress::revoke_api_key_handler,
    ),
    components(schemas(
        Address,
        HexBytes,
        HexUint256,
        CallData,
        TransitionIndex,
        GasKillerTaskRequest,
        GasKillerTaskRequestBody,
        TaskAcceptedResponse,
        TaskView,
        TaskStatus,
        PayloadView,
        AvsMetadata,
        AvsContracts,
        AvsOperatorSetMetadata,
        AvsOperatorSetSoftware,
        CreateApiKeyRequest,
        CreatedApiKey,
        ApiKeyMetadata,
        ApiErrorEnvelope,
        ApiErrorBody,
        ErrorCode,
    )),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

/// Renders the document to the exact bytes `router/openapi.json` holds: pretty-printed JSON with
/// a trailing newline, so the committed file is a well-formed text file and comparing the two is
/// a plain string equality.
pub fn render() -> Result<String, serde_json::Error> {
    Ok(format!("{}\n", ApiDoc::openapi().to_pretty_json()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed document is what the docs site consumes, so it has to be regenerated
    /// whenever a handler annotation or a DTO changes. This is the gate that makes forgetting
    /// visible, in CI as well as locally.
    #[test]
    fn committed_document_matches_the_handlers() {
        let committed = include_str!("../openapi.json");
        let generated = render().expect("the document serializes");
        assert_eq!(
            committed, generated,
            "router/openapi.json is stale; regenerate it with `cargo run --bin openapi`"
        );
    }

    /// Guards the representations a structural derive would get wrong, which is the whole reason
    /// the schemas above are hand-built. A regression here means the document promises a shape
    /// the handlers do not serve.
    #[test]
    fn wire_representations_survive_generation() {
        let doc = serde_json::to_value(ApiDoc::openapi()).expect("the document serializes");
        let schemas = &doc["components"]["schemas"];

        // Addresses and uint256s are hex strings, not byte arrays or JSON numbers.
        assert_eq!(schemas["Address"]["type"], "string");
        assert_eq!(schemas["HexUint256"]["type"], "string");
        assert_eq!(schemas["HexBytes"]["type"], "string");

        // `call_data` is the one byte sequence that is an array rather than a hex string.
        assert_eq!(schemas["CallData"]["type"], "array");
        assert_eq!(schemas["CallData"]["items"]["maximum"], 255);
        assert_eq!(schemas["CallData"]["minItems"], 4);

        // `transition_index` keeps all three accepted JSON types.
        let variants = schemas["TransitionIndex"]["anyOf"]
            .as_array()
            .expect("the union is rendered as anyOf");
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[1]["enum"][0], "auto");
        assert_eq!(variants[2]["type"], "null");

        // Error codes stay SCREAMING_SNAKE_CASE, which integrators match on.
        let codes = schemas["ErrorCode"]["enum"]
            .as_array()
            .expect("the error codes are an enum");
        assert!(codes.contains(&serde_json::json!("TRANSITION_MISMATCH")));
        assert!(codes.contains(&serde_json::json!("PAYLOAD_EXPIRED")));

        // Task statuses stay snake_case.
        let statuses = schemas["TaskStatus"]["enum"]
            .as_array()
            .expect("the statuses are an enum");
        assert!(statuses.contains(&serde_json::json!("queued")));
        assert!(statuses.contains(&serde_json::json!("ready")));
    }

    /// Every route `build_app` serves has to appear in the document, or the generated spec is
    /// silently narrower than the API. Paths are listed by hand in the `paths(...)` attribute,
    /// which is exactly the list that can drift.
    #[test]
    fn every_served_route_is_documented() {
        let doc = serde_json::to_value(ApiDoc::openapi()).expect("the document serializes");
        let paths = doc["paths"].as_object().expect("the document has paths");

        // Written in OpenAPI's `{param}` form rather than axum's `:param`.
        for (path, methods) in [
            ("/healthz", &["get"][..]),
            ("/avs-metadata", &["get"][..]),
            ("/tasks", &["post", "get"][..]),
            ("/tasks/{task_id}", &["get"][..]),
            ("/admin/keys", &["post", "get"][..]),
            ("/admin/keys/{id}", &["delete"][..]),
        ] {
            let item = paths
                .get(path)
                .unwrap_or_else(|| panic!("{path} is served but not documented"));
            for method in methods {
                assert!(
                    item.get(method).is_some(),
                    "{method} {path} is served but not documented"
                );
            }
        }
    }
}
