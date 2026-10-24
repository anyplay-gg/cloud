# Configuration

Both programs read a TOML file and let environment variables override every value. An empty variable counts as not set. Unknown settings in the file are an error, so a typo does not go unnoticed.

- The cloud (`anyplay-cloud`): `--config <file>` or `ANYPLAY_CLOUD_CONFIG` names the file. Variables are `ANYPLAY_CLOUD_<NAME>`, with the setting name in upper case.
- The relay (`anyplay-relay`): `--config <file>` or `ANYPLAY_RELAY_CONFIG`. Variables are `ANYPLAY_RELAY_<NAME>`.

Without a file, the defaults apply. The cloud runs that way for development: sign-in codes go to the log, nothing is encrypted and anyone can sign up.

## The cloud

| Setting | Default | Meaning |
|---|---|---|
| `listen` | `0.0.0.0:8080` | The address the HTTP server binds. It speaks plain HTTP; put a TLS proxy such as Caddy in front. It also answers `/healthz` (the process runs) and `/readyz` (the node can take players: its store answers, NATS is connected when it has one, and it is not draining). |
| `metrics_listen` | none | An address such as `0.0.0.0:9090` where `/metrics` answers in the Prometheus text format: event connections, requests by route and status with their time, news between nodes, sweeps and store errors. Keep it off the internet. |
| `drain_seconds` | `5` | When told to stop, the node reports itself not ready for this long, so a load balancer stops sending players, then closes its event connections with 1001; clients come back spread over ten seconds. |
| `log_format` | `text` | `json` writes one JSON object per line, for log collectors. |
| `data_dir` | `data` | The folder with the database (`anyplay-cloud.db`) and the signing key (`signing.key`, with the public half in `signing.pub`). The key is created on first start. Back this folder up. |
| `signing_key_file` | none | A file that holds the signing key in the form `anyplay-cloud keygen` writes (32 bytes in base64). The cloud only reads it: it never creates the file, never writes `signing.pub` next to it and stops with an error if it is missing, so it can be a read-only secret mount. Without it the key lives in `data_dir`. Meant for clusters, where every node needs the same key ([Kubernetes](kubernetes.md)). `anyplay-cloud keygen` with it set prints the public key and writes nothing. |
| `database_url` | none | A PostgreSQL database instead of the file in `data_dir`, such as `postgres://cloud:secret@db.internal/cloud`. Add `?sslmode=verify-full&sslrootcert=<file>` to check the server's certificate. The cloud creates its tables on the first start. A cloud needs Postgres only when it runs as several nodes ([ADR 0003](adr/0003-postgres-for-a-cluster.md)). |
| `database_connections` | `16` | The most connections the cloud keeps open to its database. |
| `nats_url` | none | A NATS server, such as `nats://nats.internal:4222` (`tls://` for TLS). With it the cloud runs as one node of a cluster: the nodes share the database of `database_url`, which must be set, and tell each other about changes over NATS ([ADR 0004](adr/0004-nats-carries-news-not-state.md)). Every node needs the same signing key, so give them the same key: the same `signing.key` in `data_dir`, or one secret file named by `signing_key_file`. |
| `name` | `AnyPlay cloud` | The public name, shown by `GET /v1/info` and in sign-in mails. 1 to 64 characters. |
| `relays` | none | The URLs of your relays, such as `https://relay.example.com`. The cloud hands out a ticket for each. In an environment variable, separate them with commas. |
| `mail` | `log` | `log` writes sign-in codes to the log. `smtp` sends them. |
| `mail_from` | `AnyPlay cloud <noreply@localhost>` | The sender of sign-in mails. |
| `smtp_host` | none | The SMTP server. Required with `mail = "smtp"`. |
| `smtp_port` | by `smtp_tls` | `587` for `starttls`, `465` for `tls`, `25` for `none`. |
| `smtp_username`, `smtp_password` | none | Log-in for the SMTP server. Set both or neither. |
| `smtp_tls` | `starttls` | `starttls` (switch to TLS), `tls` (TLS from the first byte) or `none` (only for a server on the same machine). |
| `signup` | `open` | `closed` stops new accounts. Existing accounts can still sign in. |
| `privacy_url`, `terms_url` | none | Links the launcher shows before sign-up. |
| `trust_proxy` | `false` | Take a client's address for rate limits from the first hop of `X-Forwarded-For`. Turn it on only behind a proxy that sets the header; otherwise anyone can pick their own address. |
| `admin_token` | none | A secret of at least 16 characters. It switches on the `/admin/v1/...` routes and the `admin` commands. Without it the routes do not exist. |

The `admin` and `healthcheck` commands read the same settings to find the cloud (`listen`) and the token, so run them with the same environment.

### Sign-in mails

With `mail = "smtp"` the cloud never writes addresses or codes to its log above debug level. With `mail = "log"` it does, at info level, because that is the point of the mode.

### Ban and report commands

```text
anyplay-cloud admin ban <handle> [--days N] [--reason TEXT]
anyplay-cloud admin unban <handle>
anyplay-cloud admin reports [--limit N]
```

A ban signs the account's event connections out at once, takes it out of its sessions and stops the same e-mail address from signing up again, even after the account is deleted. Without `--days` it is for good.

### Fixed values

These come from the API specification and are not settings: a sign-in code has 6 digits, lives 10 minutes and allows 5 tries; an access token lives 15 minutes; a relay ticket 24 hours, and an account may ask for 20 of them per hour; an invitation 15 minutes; an account stays online 10 seconds after its last event connection closes and keeps its sessions for 30 seconds; a virtual address is kept 10 minutes after an account goes offline; the cloud pings every 20 seconds. At most 10 devices stay signed in per account; a new one replaces the one used least recently.

## The relay

| Setting | Default | Meaning |
|---|---|---|
| `cloud_public_key` | none | The cloud's public key as printed by `anyplay-cloud keygen`. |
| `cloud_public_key_file` | none | A file that holds the same text. The cloud writes `signing.pub` into its data folder. Set one of the two; the key wins when both are set. |
| `cloud_public_key_wait` | `120` | Seconds to wait for that file, because the cloud creates it on its first start. |
| `http_bind` | `0.0.0.0:80` | Plain HTTP. With `tls = "none"` the relay itself is served here. |
| `https_bind` | `0.0.0.0:443` | HTTPS. |
| `quic_bind` | `0.0.0.0:7842` | The UDP port for QUIC address discovery, which needs TLS. |
| `tls` | `letsencrypt` | `none` (development only), `manual` (certificate and key from files), `reloading` (the same, and the files are read again every hour) or `letsencrypt`. |
| `cert_file`, `key_file` | none | PEM files for `manual` and `reloading`. |
| `cert_wait` | `180` | Seconds to wait for those files to appear. |
| `hostname` | none | The host name of the relay, for `letsencrypt`. |
| `contact` | none | An e-mail address Let's Encrypt can reach, for `letsencrypt`. |
| `cert_dir` | `certs` | Where Let's Encrypt data is kept. |
| `letsencrypt_staging` | `false` | Use Let's Encrypt's test service. Its certificates are not trusted by any client. |
| `client_bytes_per_second` | `1048576` | What one client may send through the relay, in bytes per second (about 8 Mbit/s). Over it the relay stops reading from that client until it is within the rate, which slows the sender down. `0` turns the limit off. |
| `client_burst_bytes` | `2097152` | How much a client may send at once before the rate holds it back. `0` leaves it to iroh-relay, a tenth of a second of the rate. |
| `max_clients` | `10000` | The most clients this relay carries at once. Past it, new clients are refused with `relay is full` and try another relay. Keep it below what the open-files limit allows (see below). `0` is no limit. |
| `max_connections_per_account` | `16` | The most connections one account may hold on this relay, over all its devices. The account is read from the signed ticket. `0` is no limit. |
| `max_ticket_lifetime_secs` | `90000` | A ticket that is still valid for longer than this when the client presents it is refused. The cloud issues 24 hours; the default adds one hour of slack. `0` is no limit. |
| `metrics_bind` | none | An address such as `0.0.0.0:9091` where `/metrics` answers in the Prometheus text format. Keep it off the internet. |

### Which TLS mode

`letsencrypt` needs the relay to answer on port 443 of its host name: Let's Encrypt checks there and nowhere else. Use it when the relay has an address of its own, for example on a separate machine from the cloud's proxy.

When the relay shares an address with a proxy that already holds port 443, let the proxy fetch the certificate and use `reloading` with the proxy's certificate files, on another port such as 8443. The compose file in `deploy/` does this with Caddy. Clients reach such a relay at `https://relay.example.com:8443`, and that is the URL to list in the cloud's `relays`.

The relay admits a client only with a ticket that the cloud signed for that client's own endpoint. It checks the ticket itself with the public key and never calls the cloud, so it keeps working while the cloud restarts.

A ticket is checked once, when the client connects. A client that is connected stays until it disconnects, even after its ticket has run out; the ticket's short life only matters when the client comes back.

### Metrics

With `metrics_bind` set, `GET /metrics` answers these. No label holds an account or an endpoint ID.

| Metric | Meaning |
|---|---|
| `anyplay_relay_clients` | Clients connected now. |
| `anyplay_relay_accounts` | Accounts with a client connected now. |
| `anyplay_relay_max_clients` | The setting `max_clients`, to alert on how full the relay is. |
| `anyplay_relay_bytes_in_total`, `anyplay_relay_bytes_out_total` | Bytes received from and sent to clients. Egress is what the provider bills. |
| `anyplay_relay_packets_dropped_total` | Packets dropped because the receiver was gone. |
| `anyplay_relay_rate_limited_bytes_total`, `anyplay_relay_rate_limited_connections_total` | Bytes and connections held back by `client_bytes_per_second`. |
| `anyplay_relay_ticket_checks_total{result}` | Every connection attempt once, by `ok`, `missing`, `malformed`, `signature`, `kind`, `expired`, `endpoint` or `lifetime`. |
| `anyplay_relay_refusals_total{reason}` | Connections turned away, by `ticket` (the checks say which), `account_limit` or `relay_full`. |

Worth an alert: `relay_full` refusals above zero (add a relay), `anyplay_relay_clients` above 80 % of `anyplay_relay_max_clients`, and a rise in `signature` checks (tickets from another key, or someone guessing).

### Running a relay

**Sizing.** A relay costs bandwidth and sockets, not CPU. Every relayed packet passes through once in and once out, so a relay with 1,000 clients that each send 100 kB/s moves about 100 MB/s (800 Mbit/s) in and the same out. Plan the port speed and the provider's traffic allowance for the sum of what clients send, and watch `anyplay_relay_bytes_out_total` for the real figure; the per-client limit is a ceiling, not an average. Memory per client has not been measured yet; start with 2 GB for `max_clients = 10000` and look at the process.

**Ports.** TCP 443 (or the port in `https_bind`, such as 8443 behind Caddy) for clients, and UDP 7842 for address discovery. TCP 80 is needed only if you use `http_bind` for something; with `tls = "letsencrypt"` the certificate check runs on 443. The metrics port is never published; scrape it from inside the host or the private network.

**Open files.** Each client is a socket. Allow at least `2 * max_clients + 1024` open files: `ulimit -n 30000` for the default, `LimitNOFILE=30000` in a systemd unit, or `ulimits: nofile: 30000` in a compose file. With the usual default of 1,024 the relay runs out of sockets long before `max_clients`.

**UDP buffers.** Address discovery shares one UDP socket among all clients. Raise the kernel's buffers so bursts are not dropped: `sysctl -w net.core.rmem_max=7500000 net.core.wmem_max=7500000`, and keep it in `/etc/sysctl.d/`.

**DDoS.** The relay can refuse a client only after the TLS and WebSocket handshakes, so a flood of new connections costs it work before any limit applies; `iroh-relay` 1.4 has a setting for the rate of new connections but does not implement it. Put the volumetric filtering where it belongs: pick a provider whose DDoS protection covers TCP and UDP for the relay's address, and allow only the ports above. A per-address limit on new connections (for example `nftables` `ct count` or `limit rate` on port 443) is the next layer. Refused connections show up in the counters above and never in the log.
