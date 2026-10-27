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
| `media_dir` | `data_dir/media` | Where the screenshots and clips that players send to friends are kept, one file each under a random key (`ab/ab12...`; half-uploaded files in `tmp/`). Files expire after 30 days and the sweep deletes them. **With more than one node every node must see the same folder** (a shared volume), and it must be backed up with the database: a row without its file shows the friend a media message that cannot be opened. Env: `ANYPLAY_CLOUD_MEDIA_DIR`. |
| `media_account_bytes` | `1073741824` (1 GiB) | The most bytes of unexpired, not unsent screenshots and clips one account may have in the cloud. At least 104857600 (one clip). Env: `ANYPLAY_CLOUD_MEDIA_ACCOUNT_BYTES`. |
| `signing_key_file` | none | A file that holds the signing key in the form `anyplay-cloud keygen` writes (32 bytes in base64). The cloud only reads it: it never creates the file, never writes `signing.pub` next to it and stops with an error if it is missing, so it can be a read-only secret mount. Without it the key lives in `data_dir`. Meant for clusters, where every node needs the same key ([Kubernetes](kubernetes.md)). `anyplay-cloud keygen` with it set prints the public key and writes nothing. |
| `database_url` | none | A PostgreSQL database instead of the file in `data_dir`, such as `postgres://cloud:secret@db.internal/cloud`. Add `?sslmode=verify-full&sslrootcert=<file>` to check the server's certificate. The cloud creates its tables on the first start. A cloud needs Postgres only when it runs as several nodes ([ADR 0003](adr/0003-postgres-for-a-cluster.md)). |
| `database_connections` | `16` | The most connections the cloud keeps open to its database. |
| `database_acquire_timeout_ms` | `5000` | How long a request waits for a free database connection (on SQLite also for its turn to write) before it is answered with 429 and `Retry-After` instead of hanging. At least 100. |
| `database_statement_timeout_ms` | `15000` | PostgreSQL only: a statement that runs longer is cancelled and answered like an acquire timeout. `0` switches it off. The cloud also ends a transaction that sits idle for 30 s. Migrations, the retention sweeps and the deletion of an account run without the limit. A cancelled statement is counted in `anyplay_cloud_store_statement_timeouts_total`, not in the pool timeouts. |
| `request_timeout_secs` | `30` | A request not answered within this time gets a 503 with `Retry-After`. The event connection, `/metrics` and the health checks (`/healthz`, `/readyz`) are left out. `0` switches it off. A **write** that runs into the limit is not cancelled: it runs to its end and only the wait for its answer stops, so the 503 on a write means "not known yet", never "not done". Check (for example list the sessions) before repeating a create. A read is cancelled. |
| `max_in_flight_requests` | `0` | The most requests a node works on at once. Another gets a 503 with `Retry-After` instead of queueing; the event connection, `/metrics` and the health checks do not count and are never turned away, so a full node is not restarted for it. `0` is no limit; set it only after measuring, as a guard against a pile-up. |
| `nats_url` | none | A NATS server, such as `nats://nats.internal:4222` (`tls://` for TLS). With it the cloud runs as one node of a cluster: the nodes share the database of `database_url`, which must be set, and tell each other about changes over NATS ([ADR 0004](adr/0004-nats-carries-news-not-state.md)). Every node needs the same signing key, so give them the same key: the same `signing.key` in `data_dir`, or one secret file named by `signing_key_file`. |
| `status_web_url` | none | An address the cloud asks to see whether the website answers, such as `http://web:3000/` (any answer below 500 counts as up). It feeds the `web` part of `GET /v1/status`; without it that part is `unknown`. Env: `ANYPLAY_CLOUD_STATUS_WEB_URL`. `GET /v1/status` is public and reports `api`, `database`, `signalling`, `relay` (up when at least one of `relays` answers) and `web` as `up`, `down` or `unknown`, plus `operational`, `degraded` or `outage`, and nothing else; the answer is reused for 10 seconds. |
| `name` | `AnyPlay cloud` | The public name, shown by `GET /v1/info` and in sign-in mails. 1 to 64 characters. |
| `relays` | none | The URLs of your relays, such as `https://relay.example.com`. The cloud hands out a ticket for each. In an environment variable, separate them with commas. |
| `mail` | `log` | `log` writes sign-in codes to the log. `smtp` sends them. The cloud refuses to start with `log` unless `listen` is a loopback address (`127.0.0.1`, `[::1]`) or `allow_log_mail` is set. |
| `allow_log_mail` | `false` | Allows `mail = "log"` on a listen address that is not a loopback address, for a development cloud in a container. Environment variable `ANYPLAY_CLOUD_ALLOW_LOG_MAIL`. Never on a cloud with real players. |
| `mail_from` | `AnyPlay cloud <noreply@localhost>` | The sender of sign-in mails. |
| `smtp_host` | none | The SMTP server. Required with `mail = "smtp"`. |
| `smtp_port` | by `smtp_tls` | `587` for `starttls`, `465` for `tls`, `25` for `none`. |
| `smtp_username`, `smtp_password` | none | Log-in for the SMTP server. Set both or neither. |
| `smtp_tls` | by `smtp_port` | `starttls` (plain start, then switch to TLS; port 587), `tls` (TLS from the first byte, also written `implicit`; port 465, SMTPS) or `none` (only for a server on the same machine). Not set: `tls` when `smtp_port` is `465`, `starttls` on any other port. A mode that does not fit the port (`starttls` on 465, `tls` on 587) is logged as a warning at start. |
| `signup` | `open` | `closed` stops new accounts. Existing accounts can still sign in. |
| `privacy_url`, `terms_url` | none | Links the launcher shows before sign-up, and the account web in its footer and next to the terms checkbox. |
| `terms_version` | none | The version of the terms in force, such as `2026-10` (at most 64 characters). The account web asks a new player to accept it and records it with the time; a player who accepted another version, or none, is asked again at their next sign-in on the web. Without it the web asks nothing and no version is recorded. |
| `public_url` | none | The address players reach the cloud at, as an origin: `https://play.example.com`, without path. It is the origin of the account web (pages to sign in, create an account and manage it, under `/web`) and the relying party of passkeys, so a passkey is tied to its host name and stops working if you change it. It must have a domain name, not an IP address, and be `https` (`http` only for `localhost`). `GET /v1/info` announces it as `web`. Without it there is no account web and no passkeys; the launcher then falls back to the e-mail code. |
| `portal_dir` | none | The folder with the built web portal, a single-page app: its `index.html` and its `assets/`. With it the cloud serves the portal from its own origin (`/`, `/sign-in`, `/create`, `/account/...`, `/friends`, `/trophies`, `/u/<handle>`) and sends the app that signs in for the launcher to it instead of to the HTML pages. Needs `public_url`, and the folder must hold `index.html`. The portal talks to the JSON API under `/web/api/v1` ([spec](https://github.com/anyplaynet/network/blob/main/spec/portal-api.md)). Environment variable `ANYPLAY_CLOUD_PORTAL_DIR`. |
| `portal_external` | `false` | The account web is served by another service on the cloud's origin, for example [anyplaynet/web](https://github.com/anyplaynet/web) behind Caddy (`deploy/compose.web.yaml`). The cloud then sends the app that signs in for the launcher to `/sign-in`, `/create` and `/account`, and the link of its account button (`POST /v1/auth/browser/link`) to `/link` (the old `/web/link` answers with a redirect there), but serves none of those paths itself. Needs `public_url`; not together with `portal_dir`. Environment variable `ANYPLAY_CLOUD_PORTAL_EXTERNAL` (`true` or `false`). |
| `legacy_device_days` | `30` | How many days a device may sign in **without a device key**, and so refresh without a proof ([ADR 0016 of the network](https://github.com/anyplaynet/network/blob/main/docs/adr/0016-accounts-are-managed-on-the-web.md)). Launchers from before device keys need this; `0` requires a key from every device. Devices that were signed in when the cloud was upgraded to this version have 30 days from that moment, whatever this says (see below). At most 365. |
| `refresh_replay_grace_secs` | `120` | For how many seconds after a refresh token was replaced the device may ask again with it. A refresh answer that gets lost leaves the client with the old token; if the request comes with a valid device proof, the token is the one just replaced and it is within this time, the cloud treats it as a lost answer and issues new tokens instead of signing the device out (it logs `a refresh token was replayed after a lost answer`). Any other reuse of a replaced token signs the device out as before; a request without a valid proof is refused and signs nobody out. `0` treats every reuse as theft. At most 3600. Env: `ANYPLAY_CLOUD_REFRESH_REPLAY_GRACE_SECS`. |
| `trust_proxy` | `false` | Take a client's address for rate limits from the first hop of `X-Forwarded-For`. Turn it on only behind a proxy that sets the header; otherwise anyone can pick their own address. |
| `message_retention_days` | `30` | How many days the cloud keeps messages. The sweeps delete older ones, on every node. At least 1. |
| `feed_retention_days` | `90` | How many days the cloud keeps feed items (trophies earned, new friends, first plays, parties) with their reactions. The sweeps delete older ones, on every node. At least 1. A player's titles played stay remembered, so a first play is not made twice. |
| `join_code_minutes` | `120` | How long a join code lives. A join code lets a player's Discord friends ask to join their game ([Join codes](https://github.com/anyplaynet/network/blob/main/spec/cloud-api.md#join-codes)); the launcher asks for a new one before the old one ends. 1 to 10080 (a week). |
| `share_enabled` | `false` | Offer share play: a host streams the game they run to one member of their party ([spec](https://github.com/anyplaynet/network/blob/main/spec/share-play.md)). Off, every share route answers `forbidden` and `GET /v1/info` says `share.enabled: false`. Keep it off on a public cloud until the legal review of share play is done. Env: `ANYPLAY_CLOUD_SHARE_ENABLED` (`true` or `false`). `deploy/compose.yaml` does not pass it on, so a compose deployment needs it added to the `cloud` service's `environment`. |
| `leaderboards_enabled` | `false` | Offer leaderboards: boards of best scores per title and board ID that games record to and read from, and the read-only board pages of the web ([spec](https://github.com/anyplaynet/network/blob/main/spec/cloud-api.md#leaderboards), [ADR 0041](https://github.com/anyplaynet/network/blob/main/docs/adr/0041-leaderboards-are-boards-of-best-scores-named-by-the-game.md)). Off, every score and board route answers `forbidden` and the web's reads `not_found`; the account's audience setting still works. Keep it off on a public cloud until the privacy notice names scores. Env: `ANYPLAY_CLOUD_LEADERBOARDS_ENABLED` (`true` or `false`). |
| `share_relay_kbps` | `6000` | The most a relayed share should stream, in kbit/s. Announced as `share.relay_kbps` in `GET /v1/info`; the net service holds its encoder to it. 100 to 1000000. Env: `ANYPLAY_CLOUD_SHARE_RELAY_KBPS`. |
| `share_relay_minutes_per_day` | `120` | Minutes per UTC day that one host's shares may run over the relay. A share past it ends with the reason `quota`. 1 to 1440. Env: `ANYPLAY_CLOUD_SHARE_RELAY_MINUTES_PER_DAY`. |
| `admin_token` | none | A secret of at least 16 characters. It switches on the `/admin/v1/...` routes and the `admin` commands, and lets the operator call `/v1/moderation/...` with it. It is the bootstrap for naming the first moderators; whatever it does is logged as `operator`. Without it those routes do not exist. |

The `admin` and `healthcheck` commands read the same settings to find the cloud (`listen`) and the token, so run them with the same environment.

### Sign-in mails

With `mail = "smtp"` the cloud never writes addresses or codes to its log above debug level. With `mail = "log"` a sign-in code and its address are written at info level, because that is the point of the mode; that is why the cloud refuses the mode on a public address without `allow_log_mail`. A notice mail (deletion, ban) is logged with its subject only, never the address.

When a mail cannot be sent, the cloud logs one line at error level, such as `cannot send a sign-in code class=auth status=535`, and the player sees the same generic error as before. `class` is one of:

| Class | Meaning |
|---|---|
| `connect` | No connection: DNS, refused, unreachable or dropped. |
| `tls` | The TLS handshake failed, or the server wants a different `smtp_tls` (for example "must issue a STARTTLS command first"). |
| `auth` | The server refused `smtp_username` or `smtp_password` (530, 534, 535, 538 or 454). |
| `rejected_recipient` | The server refused the address (550 to 554). A best guess from the reply. |
| `rejected_sender` | The server refused `mail_from` (550 to 554 with the sender named). A best guess from the reply. |
| `invalid_recipient` | The address is not an address. Nothing was sent. |
| `timeout` | The server or the cloud ran out of time (15 seconds per SMTP step, 20 seconds per mail). |
| `other` | Anything else. `status` still says what the server answered, if it did. |

The line carries the SMTP `status` when the server answered with one, and `reply`, the server's own words with every e-mail address taken out (such as `domain is not verified`). It never carries the address, the code or a secret. A mail the server accepted is logged at info level as `sign-in code accepted by the mail server`; if that line appears and the mail never arrives, the problem is behind the server (sender domain, spam filtering), not in the cloud. To try the settings without a player, run `anyplay-cloud admin mail-test <address>` (needs `admin_token`); it sends one short mail through the running cloud and prints `sent` or `failed: <class> (SMTP <status>)` and the server's reply.

### Retiring old launchers

| Setting | Default | Meaning |
|---|---|---|
| `min_launcher_version` | none | The oldest launcher version the API accepts, such as `0.3.0` (plain `major.minor.patch`, optionally with a pre-release). A launcher below it is refused with `client_too_old` (HTTP 426, body `minimum`) on every `/v1/` request, the event connection included, except `GET /v1/info`; the launcher shows its "Update required" screen. Empty or unset: no launcher is refused. A value that is not a version stops the start. The environment variable is `ANYPLAY_CLOUD_MIN_LAUNCHER_VERSION`; `deploy/.env` calls it `MIN_LAUNCHER_VERSION`. |

Only a client that sends `X-AnyPlay-Client: launcher/<version>` is checked ([cloud API](https://github.com/anyplaynet/network/blob/main/spec/cloud-api.md#conventions)). Browsers, the account web, the net service on its own, tools, and launchers that send nothing keep working; `/healthz`, `/readyz`, `/metrics` and the `/web` paths are never under the gate. Refused requests show in `anyplay_cloud_http_requests_total` with `status="426"`.

To retire old launchers:

1. Raise it only after the release that fixes things has been out for a while, so that players have been offered the update through the launcher's own updater. Keep it aligned with the launcher's signed `minimum_version` (the launcher repository's `UPDATE_MINIMUM_VERSION`, see its `docs/UPDATES.md`): a launcher below that number cannot update itself from the manifest and needs the installer, so a cloud minimum well above it sends such players to an update screen their launcher cannot act on by itself.
2. Set `MIN_LAUNCHER_VERSION=<version>` in `cloud/deploy/.env` (on a Helm deployment, `config.minLauncherVersion`; in a file, `min_launcher_version`).
3. Restart the cloud: `cloud/deploy/update.sh` recreates the container with the new value (see [deploy.md](deploy.md)). Check with `curl -s -H 'X-AnyPlay-Client: launcher/0.0.1' https://<host>/v1/friends`, which answers 426 and the minimum.
4. To turn it off, empty the variable (or remove the line) and run `deploy/update.sh` again. Requests pass again at once after the restart.

### Moderation settings

The model, the commands and what players see are in [moderation.md](moderation.md). The settings:

| Setting | Default | Meaning |
|---|---|---|
| `ban_secret` | none | A secret of at least 16 characters for the keyed hash of banned e-mail addresses. Without it the cloud makes a random key in `data_dir/ban.key`. All nodes of a cluster need the same one; back it up. |
| `new_account_hours` | `24` | An account is new for this long. A new account cannot start a conversation with someone who is not its friend, and may send only `new_account_friend_requests_per_day` friend requests. `0` turns the rule off. |
| `new_account_friend_requests_per_day` | `10` | Friend requests an account may send while it is new (see `new_account_hours`). A muted account sends none. |
| `appeal_days` | `183` | How long after an action the player may appeal (6 months, the DSA minimum for the internal complaint system). |
| `report_limit_per_day` | `20` | Reports one account may file in 24 hours. |
| `retention_bans_days` | `183` | Days a temporary ban, a mute, a warning, a lifted action and their appeals are kept after they end (after they were given, for a warning). Not shorter than `appeal_days`. A ban for good is kept until lifted. |
| `retention_reports_days` | `183` | Days a decided report is kept after the decision. |
| `retention_open_reports_days` | `365` | Days after which a report nobody decided is deleted. |
| `retention_audit_days` | `1095` | Days a line of the audit log is kept. |

The sweep that deletes expired data runs on every node about once an hour.

### Moderation commands

```text
anyplay-cloud admin reports [--status open|decided|all] [--limit N]
anyplay-cloud admin dismiss <report-id> --reason TEXT
anyplay-cloud admin warn <handle> --reason TEXT [--rule RULE] [--report ID]
anyplay-cloud admin mute <handle> --days N --reason TEXT [--rule RULE] [--report ID]
anyplay-cloud admin ban <handle> --reason TEXT [--days N] [--rule RULE]
anyplay-cloud admin unban <handle>
anyplay-cloud admin grant-moderator <handle>
anyplay-cloud admin revoke-moderator <handle>
anyplay-cloud admin moderators
anyplay-cloud admin appeals
anyplay-cloud admin decide-appeal <appeal-id> [--reverse] --reason TEXT
anyplay-cloud admin log [--limit N]
```

A ban signs the account's event connections out at once, takes it out of its sessions and stops the same e-mail address from signing up again, even after the account is deleted. Without `--days` it is for good. Every action needs a reason, which the player reads.

### Account web, passkeys and device keys

With `public_url` set, the cloud serves pages under `/web`: sign in (by passkey or e-mail code), create an account (address, code, handle, age, terms), and the account page (e-mail, passkeys, devices, "sign out everywhere", a copy of the data, deleting the account). The launcher opens them in the system browser. They come with the binary: server-rendered HTML, one small script for passkeys, the font Inter (SIL OFL, `/web/assets/inter-OFL.txt`) and a strict content security policy that allows nothing from another origin.

- Put a TLS proxy in front, as for the API: the session cookie is `Secure` and named `__Host-anyplay` when `public_url` is `https`. Pass `Host` and `X-Forwarded-For` on. The cookie is the only one the cloud sets. It is strictly necessary for signing in, lives at most 4 hours (30 minutes unused), and is gone when the browser session ends.
- A player who is banned and proves who they are (code or passkey) sees the statement of reasons on `/web/banned` and can file their appeal there; an address that is banned gets no new account on the web either (the keyed hash of moderation, [moderation.md](moderation.md)).
- Browser sessions, passkey ceremonies and one-time codes are in the store, so any node of a cluster can answer any page.
- A player who loses a passkey signs in with an e-mail code and removes it. A cloud with `mail = "log"` cannot send codes, so it is for development only.
- **Upgrading** runs migration 0008: every device that is signed in gets `legacy_until` = 30 days from the upgrade. Until then it refreshes as before; the net service of this version binds it to a key on its first refresh. After that date a device without a key gets `unauthorized` and signs in again. To change the date: `UPDATE devices SET legacy_until = <unix seconds> WHERE device_key IS NULL;`.
- **Upgrading a cluster**: access tokens are now `v2`; nodes of the old version refuse them. Drain and replace the nodes together, or run the new version only after the old one has stopped.
- The build links OpenSSL statically (`webauthn-rs`; the `vendored` feature). It needs Perl and a C compiler to build, which the container build has.

### Connections (Discord and Steam)

A player can link a Discord and a Steam account in the portal (`/account/connections`), see and unlink them, and choose who sees them on their profile. The cloud keeps the provider's ID, a display name and a picture address, never a token ([spec](https://github.com/anyplaynet/network/blob/main/spec/portal-api.md#connections)). Both need `public_url`.

| Setting | Default | Meaning |
|---|---|---|
| `discord_client_id` | none | The Client ID of your Discord application. With the secret it turns the Discord connection on; without them Discord is `available: false`. The ID is a number. Set it together with `discord_client_secret`. Environment variable `ANYPLAY_CLOUD_DISCORD_CLIENT_ID`. |
| `discord_client_secret` | none | The Client Secret of that application. A secret: keep it in the environment or a secret mount, not in a file in a repository. Environment variable `ANYPLAY_CLOUD_DISCORD_CLIENT_SECRET`. |
| `steam_api_key` | none | A Steam Web API key. Steam needs no setting to link (OpenID); with the key the connection also holds the persona name and picture, without it only the SteamID64. A secret. Environment variable `ANYPLAY_CLOUD_STEAM_API_KEY`. |

To switch it on for a cloud at `https://anyplaynet.com`:

1. **Discord.** Open the [Discord Developer Portal](https://discord.com/developers/applications), create an application (any name; players see it on the consent page), and open *OAuth2*. Add the redirect URL `https://anyplaynet.com/web/links/discord/callback`, exactly, with no trailing slash. Copy the *Client ID* and reset and copy the *Client Secret*. The cloud asks for the scope `identify` only; nothing else needs enabling.
2. **Steam.** Get a Web API key at <https://steamcommunity.com/dev/apikey> with a Steam account that is not limited (it needs a purchase of at least 5 USD on Steam). The domain it asks for is `anyplaynet.com`. Linking works without the key, but then the profile shows an ID instead of a name and no picture.
3. Put the three values into the environment of the cloud (`ANYPLAY_CLOUD_DISCORD_CLIENT_ID`, `ANYPLAY_CLOUD_DISCORD_CLIENT_SECRET`, `ANYPLAY_CLOUD_STEAM_API_KEY`) and restart it. `GET /web/api/v1/me/links` then lists Discord with `available: true`.
4. Check it with a test account: link both, open the profile as a friend and as a stranger, unlink both.

Notes:

- The browser reaches the providers from the player's machine, and the cloud calls `discord.com`, `steamcommunity.com` and `api.steampowered.com` itself, so it needs outbound HTTPS to them. The portal's content security policy allows pictures from `cdn.discordapp.com` and `*.steamstatic.com`.
- A player who changes the application's secret in the Developer Portal breaks new Discord links until the cloud has the new one; existing connections are unaffected, since nothing is refreshed.
- A start waits 5 minutes for the provider. An account may start 10 links per hour.

### Notifications (Web Push)

Players can turn on notifications in the portal (`/account/notifications`). The cloud then sends a push to their browsers when they get a friend request, a party or game invitation, or a message ([spec](https://github.com/anyplaynet/network/blob/main/spec/portal-api.md#notifications-web-push)). A push names who wrote and never holds the text of a message. Without the two settings below the cloud sends nothing, `GET /web/api/v1/push` answers `enabled: false`, and the portal hides the switch.

| Setting | Default | Meaning |
|---|---|---|
| `vapid_private_key` | none | The cloud's VAPID private key: 32 bytes in unpadded base64url. A secret: keep it in the environment or a secret mount, never in a file in a repository. The public key is derived from it. Environment variable `ANYPLAY_CLOUD_VAPID_PRIVATE_KEY`. |
| `vapid_subject` | none | A contact the push services can use to reach the operator, `mailto:ops@example.com` or `https://example.com/contact`. Set it together with the key. Environment variable `ANYPLAY_CLOUD_VAPID_SUBJECT`. |

To switch it on:

1. Make a key pair: `anyplay-cloud vapid-keys` (in the container: `docker compose run --rm cloud vapid-keys`). It prints the two environment lines and the public key, and stores nothing.
2. Put `ANYPLAY_CLOUD_VAPID_PRIVATE_KEY` and `ANYPLAY_CLOUD_VAPID_SUBJECT` into the environment of the cloud and restart it. `GET /web/api/v1/push` then answers `enabled: true` with the public key.
3. **Do not change the key later.** A subscription is bound to the key it was made with; with another key the push services refuse every send, and each subscription is removed after five failures. Players turn notifications on again.
4. Check it with a test account: turn notifications on in the portal, then send that account a friend request from another one.

Notes:

- The cloud calls the push services of the browsers itself (`fcm.googleapis.com`, `updates.push.services.mozilla.com`, `*.push.apple.com`, `*.notify.windows.com`), so it needs outbound HTTPS to them. A subscription for any other address is refused, so a player cannot aim the cloud at an internal address.
- A subscription the push service reports as gone (404 or 410) is deleted at once; one that fails five times in a row is deleted too. An account may hold 10. Subscriptions are deleted with the account, and are in the export (`notification_devices`: name, kinds, date; never the keys or the address).
- Metrics: `anyplay_cloud_push_sent_total`, `anyplay_cloud_push_failed_total` and `anyplay_cloud_push_pruned_total`.
- The service worker the browser registers is served by the web app, not by the cloud. It must be reachable at `/sw.js` on the cloud's origin (see `deploy.md`).

### Fixed values

These come from the API specification and are not settings (a party member that stays disconnected for 15 minutes leaves its party; a party has 8 members, a group conversation 32, a message 2,000 characters; an account may send 30 messages a minute, make 60 edits, unsends and reactions a minute, and open 10 conversations an hour with people who are not its friends): a sign-in code has 6 digits, lives 10 minutes and allows 5 tries; an access token lives 15 minutes; a proof from a device key is accepted for 60 seconds and once; the one-time code that hands a browser sign-in to the app, and a link from the app to the account web, live 60 seconds and are used once; a passkey ceremony may take 5 minutes; a browser session lives 30 minutes unused and 4 hours at most, and removing a passkey, adding one, signing out everywhere or deleting the account needs a sign-in from the last 15 minutes; an account may have 10 passkeys; a relay ticket 24 hours, and an account may ask for 20 of them per hour; an invitation 15 minutes; an account stays online 10 seconds after its last event connection closes and keeps its sessions for 30 seconds; a virtual address is kept 10 minutes after an account goes offline; the cloud pings every 20 seconds. At most 10 devices stay signed in per account; a new one replaces the one used least recently.

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

### Discord Social SDK tokens

The same Discord application also serves the net service's Social SDK ([ADR 0022](https://github.com/anyplaynet/network/blob/main/docs/adr/0022-discord-social-sdk.md)). The cloud is its confidential client: `POST /v1/me/discord/token` and `/v1/me/discord/refresh` trade the SDK's authorisation code or a refresh token for Discord's tokens with the Client Secret and hand them straight back, keeping nothing ([spec](https://github.com/anyplaynet/network/blob/main/spec/cloud-api.md#discord-social-sdk)). No new setting: the routes exist when `discord_client_id` and `discord_client_secret` are set (they answer `not_found` otherwise). In the Developer Portal the *Public Client* switch stays off, and the redirects `https://anyplaynet.com/web/links/discord/callback` and `http://127.0.0.1/callback` are both registered. An account may make 10 such trades per hour.
