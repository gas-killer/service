# Deployed image provenance (default-ns demo fleet)

The helm chart defaults to GitHub-built ghcr images (`ghcr.io/breadchaincoop/*`,
`pr-NNN` tags from service-repo Docker CI). The LIVE fleet overrides them (see
`helm/gas-killer/default-live-overrides.yaml`) with two Artifact Registry images
built via Cloud Build. Everything below is reproducible from pushed branches.

## node-fast (`us-east4-docker.pkg.dev/gas-killer-testnet/gk-fast/node-fast:v6`)
- Cross-repo build: `node-fast.Dockerfile` (3-stage: LLVM-22 + gk-fast-view revmc
  sidecar build; gas-killer-node build; slim runtime with both binaries and
  `GK_FAST_VIEW_BIN` set) over a context of `gk-fast-view/` (gas-analyzer repo,
  branch `ron/local-execution` @ 541c9e7, `crates/gk-fast-view` — verified
  byte-identical to the archived build source) + `service/` (this repo).
- Rebuild: `TAG=v7 ANALYZER=<gas-analyzer checkout> deploy/images/build-node-fast.sh`
- Deployed build: Cloud Build `b914e52e-bf4f-48a4-989d-bffada5d3e2e` (2026-07-16),
  archived source `gs://gas-killer-testnet_cloudbuild/source/1784213508.731159-*.tgz`.

## router-live (`us-east4-docker.pkg.dev/gas-killer-testnet/gk-fast/router-live:v1`)
- Built from the TRACKED `router/Dockerfile` with this repo as context (adds the
  `/shard/active` progress API, branch state ~85a54d4):
  `gcloud builds submit . --project gas-killer-testnet --config` equivalent of
  `docker build -f router/Dockerfile -t .../gk-fast/router-live:v1 .` with
  `DOCKER_BUILDKIT=1` (needed for the Dockerfile's cache mounts). `.gcloudignore`
  keeps `target/` etc. out of the upload.
- Deployed build: Cloud Build `06c3685b-762b-4e56-977c-7f0d72c486d3` (2026-07-16),
  archived source `gs://gas-killer-testnet_cloudbuild/source/1784244037.207073-*.tgz`.
