# Supply chain

What is pinned in this repository, how updates arrive, what CI says about the container images, and what is not covered yet. The cloud has no public release or registry image yet ([ARCHITECTURE.md](ARCHITECTURE.md)); the images are built in CI and by operators with [deploy/Dockerfile](../deploy/Dockerfile).

## What is pinned

| What | How |
|---|---|
| GitHub Actions | Every third-party action is pinned by full commit SHA with the release it was taken from in a `# vX.Y.Z` comment. The SHA is looked up from the release tag with `gh api repos/<owner>/<action>/git/ref/tags/<tag>`. |
| Workflow permissions | `contents: read` for every workflow; nothing here writes to the repository. |
| Checkouts | `persist-credentials: false`. Nothing in these workflows pushes. |
| Base images | `rust`, `debian` and `caddy` in the Dockerfile, and `postgres`, `nats` and `curl` in the workflows, are pinned by the digest of their multi-platform index, with the tag in a comment. The image build uses `cargo build --locked`. |
| Tools downloaded in CI | `helm`, `kubeconform` and Trivy are installed at an exact version; kubeconform and Trivy are checked against a SHA-256 in the workflow before they run. `cargo-fuzz` is installed with `--locked` at an exact version. |
| Licences, advisories, sources | `cargo deny` with [deny.toml](../deny.toml) on every pull request (the shared `rules` workflow) and every Monday (`advisories.yml`). The licences must allow use under AGPL-3.0; unknown registries and git sources are refused. |

The shared `rules` workflow comes from `anyplay-gg/.github@main`, which is the organisation's own code and is not pinned to a commit. The Helm chart's test fixtures under `deploy/helm` (a plain PostgreSQL and the NATS chart) are not digest-pinned yet.

## How updates arrive

[Dependabot](../.github/dependabot.yml) opens one pull request a week for the actions (all in one), one for the crates of the workspace and of `fuzz/` (minor and patch updates in one; a major update by itself) and one for the base images of the Dockerfile, where it changes tag and digest together. It waits a week after a release before it proposes it, so a release that is pulled or found malicious in its first days never reaches us. Security updates ignore that wait.

The workspace names the network's API crate by path (`../network/crates/api`). Dependabot sees only this repository, so it may not be able to resolve the crates and report an error instead of a pull request; update the crates by hand (`cargo update`, then the checks CI runs) until that dependency is a version.

## The images

CI builds the three images (`cloud`, `relay`, `caddy`) and does not push them, so there is no registry to attach an SBOM or a provenance attestation to. What the `docker` job does instead:

- It writes a CycloneDX SBOM of each built image and keeps them as the artifact `image-reports`.
- It runs [Trivy](https://trivy.dev/) on each image and writes the HIGH and CRITICAL findings that have a fix into the job summary.

**The scan does not block, on purpose.** Nobody has seen yet what the baseline of three Debian-based images looks like, and a new CVE in a Debian package would turn every unrelated pull request red until someone updated the base image. Look at the first reports, update the base images (Dependabot does), and accept what remains with a reason. Then make it block: in `.github/workflows/ci.yml` set `TRIVY_EXIT_CODE` of the step "SBOM and vulnerability report of the images" to `"1"`, remove its `continue-on-error`, and put the findings you accept in a `.trivyignore` file with a comment for each.

**When a workflow starts to push an image**, it should build with `docker/build-push-action` with `sbom: true` and `provenance: mode=max`, so the registry holds the SBOM and a SLSA provenance next to the image, and attest the pushed digest with `actions/attest-build-provenance` (`subject-name` the image name, `subject-digest` the build's digest, `push-to-registry: true`). That job needs `packages: write`, `id-token: write` and `attestations: write`, and nothing else. Pin those actions like all others. Operators then verify with `gh attestation verify oci://<image> --repo anyplay-gg/cloud`.

## Verify

Nothing is released yet. To check a source tree: `cargo deny --all-features check`; to check an image you built, `trivy image <image>`.

## What is not covered yet

- **Pushed images, their SBOM, provenance and signature**, as above: there is no pushing workflow.
- **Reproducible builds.** The build stage compiles on `rust:1-bookworm` (pinned by digest, which Dependabot moves) and the runtime stage runs `apt-get install ca-certificates`, which takes whatever Debian serves that day.
- **`cargo auditable` in the images**: the binaries do not carry their dependency list yet (the launcher's do).
- **Commit and tag signing**, and branch protection, are repository settings and are not switched on.
- **The Rust toolchain** in CI is `stable` as installed by `dtolnay/rust-toolchain`, not a pinned version.
