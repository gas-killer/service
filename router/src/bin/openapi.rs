//! Writes the ingress OpenAPI document to `router/openapi.json`.
//!
//! Run it after changing a handler annotation or a DTO: `cargo run --bin openapi`. The committed
//! document is what the docs site renders, and a test in [`gas_killer_router::openapi`] fails
//! while it is stale, so this is the way to make that test pass.

use anyhow::Context;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let document =
        gas_killer_router::openapi::render().context("rendering the OpenAPI document")?;

    // Resolved against the crate rather than the working directory, so the binary writes the same
    // file wherever it is invoked from.
    let destination = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
    std::fs::write(&destination, &document)
        .with_context(|| format!("writing {}", destination.display()))?;

    println!("wrote {} ({} bytes)", destination.display(), document.len());
    Ok(())
}
