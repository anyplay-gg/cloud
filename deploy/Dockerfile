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
# both together. Nothing here pushes an image: CI only builds and scans them (docs/supply-chain.md).

# rust:1-bookworm
FROM rust:1-bookworm@sha256:48b7f435b8fc6378ce43b44f15e3a47fd8919b2825f835c39bd4bd3da50af5b8 AS build
WORKDIR /src
COPY network ./network
COPY cloud ./cloud
WORKDIR /src/cloud
RUN cargo build --release --locked -p anyplay-cloud -p anyplay-relay

# node:24-bookworm-slim
FROM node:24-bookworm-slim@sha256:55fb5f5a527ea2c5c4a95f963e48ef90aa38c7b346054fdc8ee9957a2b55d205 AS web
# pnpm at the version `packageManager` in web/package.json pins; the lockfile is frozen below, and
# web/pnpm-workspace.yaml keeps the seven-day release age and the other supply-chain settings.
RUN npm install --global pnpm@12.14.0
WORKDIR /src/web
COPY cloud/web/package.json cloud/web/pnpm-lock.yaml cloud/web/pnpm-workspace.yaml cloud/web/.npmrc ./
RUN pnpm install --frozen-lockfile
COPY cloud/web ./
RUN pnpm build

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
# The built portal (web/). The cloud serves it when ANYPLAY_CLOUD_PORTAL_DIR points here and
# ANYPLAY_CLOUD_PUBLIC_URL is set (compose.yaml does both); without them it serves no portal.
COPY --from=web /src/web/dist /usr/share/anyplay/portal
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
