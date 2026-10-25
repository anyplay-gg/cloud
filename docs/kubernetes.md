# Running the cloud on Kubernetes

The Helm chart in [`deploy/helm/anyplay-cloud`](../deploy/helm/anyplay-cloud) runs the cloud as a cluster: several nodes of one program behind one Service, sharing a PostgreSQL database and a NATS server ([architecture](ARCHITECTURE.md), stage 5). A workflow ([`kubernetes.yml`](../.github/workflows/kubernetes.yml)) installs it into a kind cluster on every change and checks that it answers, also while a pod is deleted.

For one machine and a handful of players, [`docker compose`](../README.md#host-your-own) is simpler and enough.

## What you need first

| Prerequisite | Version this was tried with | Notes |
|---|---|---|
| Kubernetes | 1.29 or newer (CI: the default of kind v0.33.0) | A network plugin that enforces NetworkPolicy, or switch `networkPolicy.enabled` off. |
| Helm | 3 or 4 (CI: v4.3.1) | |
| An image | built from [`deploy/Dockerfile`](../deploy/Dockerfile), target `cloud` | No public image exists yet. Build it with the `network` checkout beside `cloud` (see the Dockerfile) and push it to a registry your cluster reads. |
| PostgreSQL | 18 | [CloudNativePG](https://cloudnative-pg.io) is the way to run it: operator chart `cloudnative-pg` **0.29.1** (operator 1.30.1). An example `Cluster` is in [`deploy/helm/examples/cnpg-cluster.yaml`](../deploy/helm/examples/cnpg-cluster.yaml). |
| NATS | 2.15 | The official chart `nats` **2.15.1** from `https://nats-io.github.io/k8s/helm/charts/` (app 2.15.1). A single server is enough to start: NATS carries only news, and losing it loses no state ([ADR 0004](adr/0004-nats-carries-news-not-state.md)). Switch on its clustering for three servers when one is not enough. |
| An Ingress or load balancer with TLS | | The cloud speaks plain HTTP. It must pass WebSockets and keep them open for hours. |
| An SMTP account | | Sign-in codes go out by mail. |
| Relays | | Not part of the chart: they need cheap bandwidth, so they run on servers of their own ([ARCHITECTURE.md](ARCHITECTURE.md#relays)). |

The chart takes PostgreSQL and NATS as addresses, not as subcharts, because their life cycle (upgrades, backups, failover) is not the cloud's.

## Install

```sh
# 1. Prerequisites (once per cluster)
helm repo add cnpg https://cloudnative-pg.github.io/charts
helm install cnpg cnpg/cloudnative-pg --version 0.29.1 -n cnpg-system --create-namespace --wait
kubectl create namespace anyplay-data
kubectl apply -n anyplay-data -f deploy/helm/examples/cnpg-cluster.yaml

helm repo add nats https://nats-io.github.io/k8s/helm/charts/
helm install nats nats/nats --version 2.15.1 -n anyplay-data \
  --set natsBox.enabled=false --set config.jetstream.enabled=false

# 2. The cloud's namespace, under the restricted Pod Security profile
kubectl create namespace anyplay
kubectl label namespace anyplay pod-security.kubernetes.io/enforce=restricted

# 3. The Secrets (below), then the chart
helm install anyplay-cloud deploy/helm/anyplay-cloud -n anyplay -f my-values.yaml
```

A `my-values.yaml` for the example above:

```yaml
image:
  repository: registry.example.com/anyplay/cloud
  tag: "0.1.0"
natsUrl: nats://nats.anyplay-data.svc.cluster.local:4222
secrets:
  existingSecret: anyplay-cloud
signingKey:
  existingSecret: anyplay-cloud-signing
config:
  name: AnyPlay
  relays: [https://relay-eu-1.example.com, https://relay-eu-2.example.com]
  smtpHost: smtp.example.com
  mailFrom: "AnyPlay <noreply@example.com>"
  privacyUrl: https://example.com/privacy
  termsUrl: https://example.com/terms
ingress:
  enabled: true
  className: nginx
  annotations:
    nginx.ingress.kubernetes.io/proxy-read-timeout: "3600"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "3600"
  hosts:
    - host: cloud.example.com
  tls:
    - secretName: cloud-tls
      hosts: [cloud.example.com]
networkPolicy:
  apiFrom:
    - namespaceSelector:
        matchLabels: {kubernetes.io/metadata.name: ingress-nginx}
  postgres:
    to:
      - namespaceSelector: {matchLabels: {kubernetes.io/metadata.name: anyplay-data}}
        podSelector: {matchLabels: {cnpg.io/cluster: anyplay-db}}
  nats:
    to:
      - namespaceSelector: {matchLabels: {kubernetes.io/metadata.name: anyplay-data}}
        podSelector: {matchLabels: {app.kubernetes.io/name: nats}}
```

The chart fails with a message when `secrets.existingSecret`, `signingKey.existingSecret`, `natsUrl` or (with `mail: smtp`) `smtpHost` is missing. Every value is in [`values.yaml`](../deploy/helm/anyplay-cloud/values.yaml) and checked by [`values.schema.json`](../deploy/helm/anyplay-cloud/values.schema.json).

## The Secrets

The chart never holds a secret. It refers to two Secrets that you create, or that a tool such as External Secrets or Sealed Secrets creates for you.

**`secrets.existingSecret`** (here `anyplay-cloud`):

| Key | Required | Content |
|---|---|---|
| `database-url` | yes | `postgres://cloud:<password>@anyplay-db-rw.anyplay-data.svc:5432/cloud`. With CloudNativePG, the operator's Secret `anyplay-db-app` has it in the key `uri`. Add `?sslmode=verify-full&sslrootcert=<file>` to check the server. |
| `smtp-username`, `smtp-password` | with a login | Both or neither. |
| `admin-token` | no | At least 16 characters. Switches on the admin routes. |

The key names can be changed in `secrets.keys`.

```sh
kubectl create secret generic anyplay-cloud -n anyplay \
  --from-literal=database-url="$(kubectl get secret anyplay-db-app -n anyplay-data -o jsonpath='{.data.uri}' | base64 -d)" \
  --from-literal=smtp-username=... --from-literal=smtp-password=...
```

### Key ceremony: the signing key

All nodes sign tokens with one Ed25519 key, and every relay checks tickets with its public half. Make the key once, on a machine you trust, with the cloud's own command:

```sh
mkdir keys && chmod 700 keys
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD/keys:/data" registry.example.com/anyplay/cloud:0.1.0 keygen
#   Created /data/signing.key.
#   Relays need this public key ...
#   <the public key: give it to the relays>

kubectl create secret generic anyplay-cloud-signing -n anyplay --from-file=signing.key=keys/signing.key
```

Then:

1. Store `keys/signing.key` in your password manager or vault, and **delete the local copy**. `signing.pub` is public and may stay.
2. Give the public key to each relay (`cloud_public_key` in its settings, [configuration](configuration.md#the-relay)).
3. Mount it as the pods do: read-only from the Secret. The chart sets `signing_key_file`, so the cloud only reads the file, never creates a key and never writes `signing.pub` next to it. A missing or unreadable file stops the node at start with an error that names the file, instead of a node signing with a key of its own.

Losing the key signs everybody out and makes every relay refuse its players until it has the new public key. A leaked key lets anyone issue tickets: replace it at once (a new key, a new Secret, then `kubectl rollout restart`, and the new public key to the relays). Rotation without signing everybody out needs a key ID in the tokens and is planned ([ARCHITECTURE.md](ARCHITECTURE.md#security)). Changing the Secret does not restart the pods by itself; run `kubectl rollout restart deployment/anyplay-cloud -n anyplay` or use a tool such as Reloader.

## What the chart sets up

- **Deployment** with `replicas` nodes (2 by default), rolling updates with `maxUnavailable: 0`, and replicas on different nodes (`topologySpreadConstraints` on the host name, and softly on the zone).
- **Settings** as `ANYPLAY_CLOUD_*` variables from `config`; secrets from the Secrets above; `log_format: json`; `metrics_listen` on the named port `metrics`.
- **Probes**: startup and liveness on `/healthz`, readiness on `/readyz`. A node told to stop reports itself not ready for `drainSeconds`, so the Service stops sending players, and then closes its connections. `terminationGracePeriodSeconds` is `drainSeconds + 25` (the drain, the 10 seconds the node waits for connections to close, and slack); the chart refuses a value that is too short.
- **Restricted profile**: `runAsNonRoot`, user and group 10001 (the image's user), no privilege escalation, all capabilities dropped, seccomp `RuntimeDefault`, a read-only root file system. With PostgreSQL and a key file the cloud writes nothing to disk; small `emptyDir` volumes at `/data` and `/tmp` are there for anything that might. The service account token is not mounted.
- **Services**: the API on port 80, and a second Service `<release>-metrics` for `/metrics`. Neither the Ingress nor the API Service carries metrics.
- **PodDisruptionBudget** (one node may be taken down at a time), optional **HorizontalPodAutoscaler**, optional **Ingress**, optional **ServiceMonitor** (Prometheus Operator).
- **NetworkPolicy**: everything is denied for the cloud's pods; then ingress to the API port (from anywhere, or from `networkPolicy.apiFrom`), ingress to the metrics port from `networkPolicy.metricsFrom` (the namespace `monitoring` by default), egress to DNS, to PostgreSQL, to NATS and, with `mail: smtp`, to the SMTP port. Point `postgres.to` and `nats.to` at the pods of your database and NATS.

## Sizing

These are starting points, not measurements; the load harness (stage 6) will replace them with numbers.

- **Nodes**: two for availability, from the first day. Add nodes for connections, not for requests: each player holds one WebSocket, and memory per connection is small. Start with the default requests (250m CPU, 256 Mi) and watch `anyplay_cloud_event_connections` per node and CPU.
- **Autoscaling** on CPU is a rough fit, since idle connections cost little. Scale-in is slow on purpose (one node per five minutes): a node that goes away makes every player on it reconnect. Do not let it scale in during an evening peak.
- **PostgreSQL**: `database_connections` (16 by default) per node, so `nodes x 16` must stay below `max_connections` with room for the operator and your tools. The example sets 200. Put a pooler (PgBouncer, which CloudNativePG can run) between them before the nodes number in the dozens. `NOTES.txt` warns when the largest node count times `config.databaseConnections` exceeds 80 percent of `config.postgresMaxConnections`. When the store is saturated a request waits at most `database_acquire_timeout_ms` (5 s) and gets a 429 with `Retry-After`, never the 30 s wait of the driver.
- **Reconnect storms** are the dangerous load ([ARCHITECTURE.md](ARCHITECTURE.md#deploys-and-failures)): after a node dies, its players come back within seconds. Keep headroom of at least one node's players on the others.
- **Ingress timeouts**: raise the idle timeout of WebSockets to an hour or more, or players are cut off every minute and come back.

## What the chart does not do

- Deploy relays, PostgreSQL, NATS, an Ingress controller, cert-manager or Prometheus.
- Back up the database. Set up CloudNativePG backups to object storage before real players arrive.
- Rotate the signing key without signing everybody out, or restart pods when a Secret changes.
- Spread over regions ([ARCHITECTURE.md](ARCHITECTURE.md#regions)). One release is one cell.
- Limit requests per client across nodes: rate limits count per node ([known limits](ARCHITECTURE.md#known-limits)).
- Publish an image. You build it from the Dockerfile.

## The CI test

[`kubernetes.yml`](../.github/workflows/kubernetes.yml) runs on pull requests that touch the chart, the cloud's code or itself, and by hand. It lints the chart (`helm lint --strict`), checks the rendered manifests with `kubeconform` against the Kubernetes schemas, builds the image with the Dockerfile, starts a kind cluster, installs NATS (the official chart, pinned) and a plain PostgreSQL ([`deploy/helm/ci`](../deploy/helm/ci)), makes the signing key with `keygen`, and installs the chart with two nodes in a namespace that enforces the restricted profile. Then it checks that both pods are ready, that `/v1/info`, `/readyz` and `/healthz` answer through the Service from another namespace, that `/metrics` answers on the metrics port and not on the API port, and that the logs are JSON. Last, it deletes one pod while a client calls `/v1/info` for 45 seconds: not one call may fail.

To run it yourself, the steps in the workflow are plain `docker`, `kind`, `helm` and `kubectl` commands.
