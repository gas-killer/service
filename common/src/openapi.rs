//! OpenAPI schemas for the Ethereum primitives that appear on the wire.
//!
//! The alloy types these stand in for do not describe themselves to utoipa, and their serde
//! representations are not what a structural derive would infer: an [`alloy_primitives::Address`]
//! is a 20-byte array in Rust but a 0x-hex string in JSON, and a [`alloy_primitives::U256`] is
//! four little-endian limbs in Rust but a 0x-hex string in JSON. Each type here carries the JSON
//! shape, and the DTO fields point at them with `#[schema(value_type = ...)]`.
//!
//! They are named after the wire form rather than the Rust type so the generated components read
//! as an API contract: a reader of the document sees `HexUint256`, not `U256`.

use utoipa::PartialSchema;
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, Schema, SchemaType, Type};

/// Builds a string schema carrying a regex the value always matches, one example, and a
/// description. Every wire primitive in this module is that same shape.
fn hex_string(pattern: &str, description: &str, example: &str) -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(SchemaType::new(Type::String))
        .pattern(Some(pattern))
        .description(Some(description))
        .examples([serde_json::json!(example)])
        .into()
}

/// A 20-byte Ethereum address, serialized as a 0x-prefixed hex string.
///
/// The pattern accepts either case: addresses are emitted lowercase by alloy's own `Serialize`
/// and EIP-55 checksummed where an integrator is expected to paste the value into Solidity
/// source, and both forms parse back.
pub struct Address;

impl PartialSchema for Address {
    fn schema() -> RefOr<Schema> {
        hex_string(
            "^0x[0-9a-fA-F]{40}$",
            "A 20-byte Ethereum address, 0x-prefixed hex.",
            "0x0000000000000000000000000000000000000001",
        )
    }
}

impl ToSchema for Address {}

/// A variable-length byte string, serialized as 0x-prefixed hex. Empty is `0x`.
pub struct HexBytes;

impl PartialSchema for HexBytes {
    fn schema() -> RefOr<Schema> {
        hex_string(
            "^0x[0-9a-fA-F]*$",
            "A 0x-prefixed hex byte string.",
            "0x93de4531",
        )
    }
}

impl ToSchema for HexBytes {}

/// A 256-bit unsigned integer, serialized as 0x-prefixed hex rather than a JSON number, which
/// could not hold it without loss.
pub struct HexUint256;

impl PartialSchema for HexUint256 {
    fn schema() -> RefOr<Schema> {
        hex_string(
            "^0x[0-9a-fA-F]+$",
            "A 256-bit unsigned integer, 0x-prefixed hex.",
            "0x0",
        )
    }
}

impl ToSchema for HexUint256 {}
