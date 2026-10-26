# Builds the cloud, the relay and a Caddy that runs as the same unprivileged user.
#
# The build context is the folder that holds both checkouts, `cloud/` and `network/` (the cloud
# depends on the network's API crate by path). From that folder:
#
#   docker build -f cloud/deploy/Dockerfile --target cloud -t anyplay-cloud .
#   docker build -f cloud/deploy/Dockerfile --target relay -t anyplay-relay .
#
# `compose.yaml` does this for you. Dockerfile.dockerignore keeps everything else out of the context.
#
# Every base image is pinned by the digest of its multi-platform index, with the tag it was taken
# from in the comment above it; Dependabot (the `docker` ecosystem in .github/dependabot.yml) moves
# both together. The `image` workflow builds the cloud and relay images from this file on every merge
# to main and pushes them to ghcr.io (docs/deploy.md, docs/supply-chain.md).
#
# The Rust build is split with cargo-chef, so that the dependencies are compiled in a layer of their
# own: `planner` writes a recipe of the manifests and the lockfile, `build` cooks it (the layer
# changes only when a dependency or `network.ref` does) and then compiles the sources on top.

# rust:1-bookworm
FROM rust:1-bookworm@sha256:48b7f435b8fc6378ce43b44f15e3a47fd8919b2825f835c39bd4bd3da50af5b8 AS chef
RUN cargo install cargo-chef --version 0.1.78 --locked
WORKDIR /src

FROM chef AS planner
COPY network ./network
COPY cloud ./cloud
WORKDIR /src/cloud
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
# The network checkout is whole here, not only the crates the cloud links: cargo needs the manifests
# of every path dependency of the workspace (the e2e crate names more of them). It changes only
# when `network.ref` does.
COPY network ./network
WORKDIR /src/cloud
COPY --from=planner /src/cloud/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json -p anyplay-cloud -p anyplay-relay
COPY cloud ./
RUN cargo build --release --locked -p anyplay-cloud -p anyplay-relay

# debian:bookworm-slim
FROM debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8 AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --user-group --no-create-home --shell /usr/sbin/nologin anyplay \
    && mkdir /data \
    && chown anyplay:anyplay /data
USER anyplay:anyplay

FROM runtime AS cloud
COPY --from=build /src/cloud/target/release/anyplay-cloud /usr/local/bin/anyplay-cloud
ENV ANYPLAY_CLOUD_DATA_DIR=/data \
    ANYPLAY_CLOUD_LISTEN=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=5s --start-period=20s --retries=3 \
    CMD ["anyplay-cloud", "healthcheck"]
ENTRYPOINT ["anyplay-cloud"]
CMD ["serve"]

FROM runtime AS relay
COPY --from=build /src/cloud/target/release/anyplay-relay /usr/local/bin/anyplay-relay
ENV ANYPLAY_RELAY_CERT_DIR=/data
EXPOSE 80 443 7842/udp
ENTRYPOINT ["anyplay-relay"]

# Caddy as user 10001, so that the relay, which runs as the same user, can read the certificates
# Caddy fetches. Caddy keeps its files in /data and /config.
# caddy:2
FROM caddy:2@sha256:504a44d7d74bd2bc28a66afc4774912fc21223ca8a67734af1c54ab2f999b489 AS caddy
RUN mkdir -p /data /config \
    && chown -R 10001:10001 /data /config
USER 10001:10001
