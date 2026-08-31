//! Writes the ingress OpenAPI documents to `router/openapi.json` and
//! `router/openapi.internal.json`.
//!
//! Run it after changing a handler annotation or a DTO: `cargo run --bin openapi`. The committed
//! documents are what the docs site consumes, and a test in [`gas_killer_router::openapi`] fails
//! while either is stale, so this is the way to make that test pass.
//!
//! The two differ by the operator surface: see [`gas_killer_router::openapi::PRIVATE_TAGS`].

use anyhow::Context;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    // Resolved against the crate rather than the working directory, so the binary writes the same
    // files wherever it is invoked from.
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    for (name, rendered) in [
        (
            "openapi.json",
            gas_killer_router::openapi::render().context("rendering the published document")?,
        ),
        (
            "openapi.internal.json",
            gas_killer_router::openapi::render_internal()
                .context("rendering the internal document")?,
        ),
    ] {
        let destination = crate_root.join(name);
        std::fs::write(&destination, &rendered)
            .with_context(|| format!("writing {}", destination.display()))?;
        println!("wrote {} ({} bytes)", destination.display(), rendered.len());
    }
    Ok(())
}
