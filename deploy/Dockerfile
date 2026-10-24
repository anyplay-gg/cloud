# Builds the cloud, the relay and a Caddy that runs as the same unprivileged user.
#
# The build context is the folder that holds both checkouts, `cloud/` and `network/` (the cloud
# depends on the network's API crate by path). From that folder:
#
#   docker build -f cloud/deploy/Dockerfile --target cloud -t anyplay-cloud .
#   docker build -f cloud/deploy/Dockerfile --target relay -t anyplay-relay .
#
# `compose.yaml` does this for you. Dockerfile.dockerignore keeps everything else out of the context.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY network ./network
COPY cloud ./cloud
WORKDIR /src/cloud
RUN cargo build --release --locked -p anyplay-cloud -p anyplay-relay

FROM debian:bookworm-slim AS runtime
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
FROM caddy:2 AS caddy
RUN mkdir -p /data /config \
    && chown -R 10001:10001 /data /config
USER 10001:10001
