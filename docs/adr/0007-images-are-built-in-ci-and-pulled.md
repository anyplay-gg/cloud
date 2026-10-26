# Images are built in CI and pulled by the server

`update.sh` used to build the images on the server: a cold `docker compose build` compiled the whole workspace for every change, the old version kept serving meanwhile, and a deploy took 20 to 27 minutes by the operator's clock (13:43 to 14:10 and 16:00 to 16:20 CET on 26.10.2026; the Rust build alone is measured at 412 to 434 s on a GitHub runner, 171 s on an 8-vCPU runner). The Dockerfile said "Nothing here pushes an image".

## Decision

The `image` workflow builds the `cloud` and `relay` targets of `deploy/Dockerfile` on every merge to `main`, pushes them to `ghcr.io/anyplaynet/cloud` and `ghcr.io/anyplaynet/relay` (tags `<commit>` and `main`), and attests each digest with `actions/attest-build-provenance`. `update.sh --image <digest> --relay-image <digest>` pulls those images by digest instead of building them. It refuses an image whose `org.opencontainers.image.revision` label is not the cloud commit it deploys, and one that `gh attestation verify` does not accept. Caddy and the website are still built on the server (small, and pinned by digest).

The build path stays: `update.sh` without `--image` works as before, for a server without access to the registry and as a fallback when the image workflow is red.

The Rust build is split with cargo-chef, so a source change compiles the sources and not the dependencies. The BuildKit layers are cached in the GitHub Actions cache, one scope per image.

## Consequences

- The server needs to read ghcr.io. The images are built from a public repository; if GitHub keeps the packages private, make them public once (package settings) or log the server in with a token that has `read:packages`.
- A deploy of an image that is already built is a pull (100 to 300 MB), the migration, the swap and the health check. The database backup, the rollback tags and the rollback itself are unchanged.
- The images are only as trustworthy as the workflow that builds them: anyone who can change `image.yml` on `main` can change what the server runs. Branch protection on `main` is the control, as for the rest of the code.
- Migrations still run on the first start of the new version and are not undone by a rollback (docs/deploy.md).
