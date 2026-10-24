# Configuration

Both programs read a TOML file and let environment variables override every value. An empty variable counts as not set. Unknown settings in the file are an error, so a typo does not go unnoticed.

- The cloud (`anyplay-cloud`): `--config <file>` or `ANYPLAY_CLOUD_CONFIG` names the file. Variables are `ANYPLAY_CLOUD_<NAME>`, with the setting name in upper case.
- The relay (`anyplay-relay`): `--config <file>` or `ANYPLAY_RELAY_CONFIG`. Variables are `ANYPLAY_RELAY_<NAME>`.

Without a file, the defaults apply. The cloud runs that way for development: sign-in codes go to the log, nothing is encrypted and anyone can sign up.

## The cloud

| Setting | Default | Meaning |
|---|---|---|
| `listen` | `0.0.0.0:8080` | The address the HTTP server binds. It speaks plain HTTP; put a TLS proxy such as Caddy in front. |
| `data_dir` | `data` | The folder with the database (`anyplay-cloud.db`) and the signing key (`signing.key`, with the public half in `signing.pub`). The key is created on first start. Back this folder up. |
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

These come from the API specification and are not settings: a sign-in code has 6 digits, lives 10 minutes and allows 5 tries; an access token lives 15 minutes; a relay ticket 24 hours; an invitation 15 minutes; an account stays online 10 seconds after its last event connection closes and keeps its sessions for 30 seconds; a virtual address is kept 10 minutes after an account goes offline; the cloud pings every 20 seconds. At most 10 devices stay signed in per account; a new one replaces the one used least recently.

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
| `client_bytes_per_second` | none | Limit what one client may send. |

### Which TLS mode

`letsencrypt` needs the relay to answer on port 443 of its host name: Let's Encrypt checks there and nowhere else. Use it when the relay has an address of its own, for example on a separate machine from the cloud's proxy.

When the relay shares an address with a proxy that already holds port 443, let the proxy fetch the certificate and use `reloading` with the proxy's certificate files, on another port such as 8443. The compose file in `deploy/` does this with Caddy. Clients reach such a relay at `https://relay.example.com:8443`, and that is the URL to list in the cloud's `relays`.

The relay admits a client only with a ticket that the cloud signed for that client's own endpoint. It checks the ticket itself with the public key and never calls the cloud, so it keeps working while the cloud restarts.
