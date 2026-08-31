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
//! The document is rendered to disk in two forms, both committed and both checked by the tests
//! below. `router/openapi.json` is the integrator-facing API and is what the docs site renders;
//! `router/openapi.internal.json` additionally carries the operator surface. See [`PRIVATE_TAGS`]
//! for what separates them and why. The `openapi` binary writes both.

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
use std::collections::BTreeSet;
use utoipa::openapi::RefOr;
use utoipa::openapi::path::Operation;
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

/// Tags whose operations describe the operator surface rather than the integrator API.
///
/// These are removed from the published document rather than merely left undocumented. The docs
/// site renders that document as an interactive playground, and `ADMIN_KEY` mints and revokes
/// every API key the router honours, so a public form is the wrong place to invite someone to
/// paste one. Removing the operations means no site-side configuration can publish them by
/// accident.
///
/// The handlers keep their annotations either way: `openapi.internal.json` is the whole API, and
/// the tests still require every served route to be documented there.
pub const PRIVATE_TAGS: &[&str] = &["Admin"];

/// The eight operation slots a path item can hold, as mutable references. `PathItem` models
/// methods as named fields rather than a map, so anything walking every operation has to spell
/// them out.
fn operation_slots(item: &mut utoipa::openapi::PathItem) -> [&mut Option<Operation>; 8] {
    [
        &mut item.get,
        &mut item.put,
        &mut item.post,
        &mut item.delete,
        &mut item.options,
        &mut item.head,
        &mut item.patch,
        &mut item.trace,
    ]
}

/// The operations a path item actually defines.
fn operations(item: &utoipa::openapi::PathItem) -> impl Iterator<Item = &Operation> {
    [
        &item.get,
        &item.put,
        &item.post,
        &item.delete,
        &item.options,
        &item.head,
        &item.patch,
        &item.trace,
    ]
    .into_iter()
    .flatten()
}

/// Whether an operation carries any of [`PRIVATE_TAGS`].
fn is_private(operation: &Operation) -> bool {
    operation
        .tags
        .as_ref()
        .is_some_and(|tags| tags.iter().any(|tag| PRIVATE_TAGS.contains(&tag.as_str())))
}

/// Names of the security schemes an operation requires.
///
/// Read back out of the serialized form because a `SecurityRequirement` keeps its scheme names in
/// a private map, and the requirement serializes to exactly `{"SchemeName": [scopes]}`.
fn required_schemes(operation: &Operation) -> Vec<String> {
    operation
        .security
        .iter()
        .flatten()
        .filter_map(|requirement| serde_json::to_value(requirement).ok())
        .filter_map(|value| match value {
            serde_json::Value::Object(map) => Some(map.into_iter().map(|(name, _)| name)),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Every `#/components/schemas/<name>` reference anywhere within `value`.
fn schema_references(value: &serde_json::Value) -> Vec<String> {
    const PREFIX: &str = "#/components/schemas/";
    let mut found = Vec::new();
    let mut pending = vec![value];
    while let Some(node) = pending.pop() {
        match node {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    if key == "$ref"
                        && let Some(name) = child.as_str().and_then(|r| r.strip_prefix(PREFIX))
                    {
                        found.push(name.to_string());
                    }
                    pending.push(child);
                }
            }
            serde_json::Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    found
}

/// Component schemas reachable from the document's remaining paths, following references through
/// the schemas themselves so a type used only by a nested field is kept.
fn reachable_schemas(document: &utoipa::openapi::OpenApi) -> BTreeSet<String> {
    let schemas = document.components.as_ref().map(|c| &c.schemas);
    let mut reachable = BTreeSet::new();
    let mut pending: Vec<serde_json::Value> =
        serde_json::to_value(&document.paths).into_iter().collect();

    while let Some(value) = pending.pop() {
        for name in schema_references(&value) {
            if reachable.insert(name.clone())
                && let Some(schema) = schemas.and_then(|s| s.get(&name))
                && let Ok(value) = serde_json::to_value(schema)
            {
                pending.push(value);
            }
        }
    }
    reachable
}

/// Removes every [`PRIVATE_TAGS`] operation from `document`, along with what only they used: the
/// security schemes they required, the component schemas they alone referenced, and their tag
/// definitions.
///
/// Pruning by reachability rather than by a list means a type added to an admin endpoint later
/// stays out of the published document without anyone remembering to exclude it.
fn strip_private(document: &mut utoipa::openapi::OpenApi) {
    for item in document.paths.paths.values_mut() {
        for slot in operation_slots(item) {
            if slot.as_ref().is_some_and(is_private) {
                *slot = None;
            }
        }
    }
    document
        .paths
        .paths
        .retain(|_, item| operations(item).next().is_some());

    let kept_schemes: BTreeSet<String> = document
        .paths
        .paths
        .values()
        .flat_map(operations)
        .flat_map(required_schemes)
        .collect();
    let kept_tags: BTreeSet<String> = document
        .paths
        .paths
        .values()
        .flat_map(operations)
        .filter_map(|operation| operation.tags.as_ref())
        .flatten()
        .cloned()
        .collect();
    let kept_schemas = reachable_schemas(document);

    if let Some(components) = document.components.as_mut() {
        components
            .security_schemes
            .retain(|name, _| kept_schemes.contains(name));
        components
            .schemas
            .retain(|name, _| kept_schemas.contains(name));
    }
    if let Some(tags) = document.tags.as_mut() {
        tags.retain(|tag| kept_tags.contains(&tag.name));
    }
}

/// Renders the integrator-facing document, the bytes `router/openapi.json` holds and the docs site
/// renders. Pretty-printed JSON with a trailing newline, so the committed file is a well-formed
/// text file and comparing the two is a plain string equality.
pub fn render() -> Result<String, serde_json::Error> {
    let mut document = ApiDoc::openapi();
    strip_private(&mut document);
    Ok(format!("{}\n", document.to_pretty_json()?))
}

/// Renders the whole API including the operator surface, the bytes `router/openapi.internal.json`
/// holds. Not for publication; see [`PRIVATE_TAGS`].
pub fn render_internal() -> Result<String, serde_json::Error> {
    Ok(format!("{}\n", ApiDoc::openapi().to_pretty_json()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed documents are what the docs site consumes, so they have to be regenerated
    /// whenever a handler annotation or a DTO changes. This is the gate that makes forgetting
    /// visible, in CI as well as locally.
    #[test]
    fn committed_documents_match_the_handlers() {
        for (committed, generated, path) in [
            (
                include_str!("../openapi.json"),
                render().expect("the published document serializes"),
                "router/openapi.json",
            ),
            (
                include_str!("../openapi.internal.json"),
                render_internal().expect("the internal document serializes"),
                "router/openapi.internal.json",
            ),
        ] {
            assert_eq!(
                committed, generated,
                "{path} is stale; regenerate it with `cargo run --bin openapi`"
            );
        }
    }

    /// The operator surface must not reach the published document, which the docs site renders as
    /// a playground. Checked on the rendered bytes rather than the in-memory document, since the
    /// bytes are what ships.
    #[test]
    fn the_published_document_excludes_the_operator_surface() {
        let published: serde_json::Value =
            serde_json::from_str(&render().expect("the published document serializes"))
                .expect("the published document is JSON");

        for path in published["paths"].as_object().expect("paths").keys() {
            assert!(
                !path.starts_with("/admin"),
                "{path} is an operator route and must not be published"
            );
        }
        for operation in published["paths"]
            .as_object()
            .expect("paths")
            .values()
            .flat_map(|item| item.as_object().expect("a path item").values())
        {
            let tags = operation["tags"].as_array().cloned().unwrap_or_default();
            for tag in tags {
                assert!(
                    !PRIVATE_TAGS.contains(&tag.as_str().unwrap_or_default()),
                    "a {tag} operation reached the published document"
                );
            }
        }

        // What only the operator surface used goes with it, so the published document carries no
        // dangling credential or type an integrator can never use.
        let components = &published["components"];
        assert!(
            components["securitySchemes"]["AdminAuth"].is_null(),
            "the admin credential is described in the published document"
        );
        for schema in ["CreateApiKeyRequest", "CreatedApiKey", "ApiKeyMetadata"] {
            assert!(
                components["schemas"][schema].is_null(),
                "{schema} is only used by the operator surface but was published"
            );
        }
        assert!(
            !published["tags"]
                .as_array()
                .expect("tags")
                .iter()
                .any(|tag| tag["name"] == "Admin"),
            "the Admin tag is defined in the published document"
        );

        // The integrator API is untouched by the pruning.
        assert!(published["paths"]["/tasks"]["post"].is_object());
        assert!(components["securitySchemes"]["ApiKeyAuth"].is_object());
        assert!(components["schemas"]["PayloadView"].is_object());
    }

    /// The internal document is the whole API, so the annotations on the admin handlers are still
    /// exercised rather than quietly rotting behind the exclusion.
    #[test]
    fn the_internal_document_keeps_the_operator_surface() {
        let internal: serde_json::Value =
            serde_json::from_str(&render_internal().expect("the internal document serializes"))
                .expect("the internal document is JSON");

        assert!(internal["paths"]["/admin/keys"]["post"].is_object());
        assert!(internal["paths"]["/admin/keys"]["get"].is_object());
        assert!(internal["paths"]["/admin/keys/{id}"]["delete"].is_object());
        assert!(internal["components"]["securitySchemes"]["AdminAuth"].is_object());
        assert!(internal["components"]["schemas"]["CreatedApiKey"].is_object());
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
