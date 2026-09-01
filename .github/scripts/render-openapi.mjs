/**
 * Fails when the published OpenAPI document cannot be rendered by the docs site's own toolchain.
 *
 * `cargo test` proves the document is current with the handlers and is valid JSON, but not that
 * anything can render it: a document can satisfy the OpenAPI schema and still break the generator
 * the site runs. This closes that gap by performing the site's own generation step against the
 * committed bytes.
 *
 * It also re-checks the operator surface at the point where publication actually happens. The Rust
 * tests assert `/admin` never reaches the published document; this asserts no page is emitted for
 * it either, so the guarantee holds at both ends of the pipeline.
 */
import { generateFiles } from "fumadocs-openapi"
import { createOpenAPI } from "fumadocs-openapi/server"
import { mkdtemp, readdir } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

const spec = process.env.OPENAPI_SPEC ?? "router/docs/openapi.json"
const output = await mkdtemp(join(tmpdir(), "openapi-render-"))

await generateFiles({
  input: createOpenAPI({ input: [spec] }),
  output,
  per: "operation",
  groupBy: "tag",
  includeDescription: true,
})

const pages = (await readdir(output, { recursive: true })).filter((file) =>
  file.endsWith(".mdx"),
)

if (pages.length === 0) {
  console.error(`${spec} rendered no pages, so the document describes no operations`)
  process.exit(1)
}

const operatorPages = pages.filter((file) => /admin/i.test(file))
if (operatorPages.length > 0) {
  console.error(
    `${spec} rendered operator pages, which must never be published:\n  ${operatorPages.join("\n  ")}`,
  )
  process.exit(1)
}

console.log(`${spec} rendered ${pages.length} pages:`)
for (const page of pages.sort()) console.log(`  ${page}`)
