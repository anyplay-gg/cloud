# Data inventory

What a cloud and its relay actually store or log about people, read from the code at the commit this pull request is based on. This is a draft for legal review, not legal advice ([README](README.md)).

How to read it:

- **Where** names the table and column, the log line or the metric.
- **Basis (proposed)** is our suggestion for the legal basis under [GDPR Art. 6(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): (b) the processing is necessary for a contract with the person, (c) a legal obligation, (f) legitimate interests, unless the person's interests override them (Art. 6(1)(f) names children as a case that weighs heavily). Every basis here is an inference for the lawyer to confirm.
- **Kept today** is what the code does. **Proposed** is what we think it should do. Where they differ, the difference is a gap in [Gaps](#gaps). Storage limitation is the rule in [Art. 5(1)(e)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): identifiable data "no longer than is necessary".
- "Operator" means whoever runs the cloud. "Moderator" means a person holding the admin token (`admin_token`); the code has no other staff role.
- Account, device and session IDs are random numbers (`util::random_id`). They are pseudonyms, not anonymous: the cloud can join them to the e-mail address ([Art. 4(1) and 4(5)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679)).

The social layer is built: trophies, profiles, avatars, messages, the feed, parties, calls, join codes, linked accounts, notifications and the 30-day deletion grace period. [Section 7](#7-the-social-layer) covers them, audited against cloud `8e0e02d` on 25 October 2026. Sections 1 to 6 were written before that and are corrected where they said something that is no longer true. Design background: [network ADR 0009](https://github.com/anyplaynet/network/blob/main/docs/adr/0009-trophy-records-not-trophy-texts.md) (trophy records), [ADR 0011](https://github.com/anyplaynet/network/blob/main/docs/adr/0011-messages-are-stored-to-be-delivered.md) (messages are stored readable by the cloud) and [ADR 0012](https://github.com/anyplaynet/network/blob/main/docs/adr/0012-private-by-default.md) (profiles start private).

## 1. The cloud database

Migrations: `crates/cloud/migrations/{sqlite,postgres}/0001_init.sql` and `0002_shared_state.sql`. A SQLite file `anyplay-cloud.db` in `data_dir`, or Postgres via `database_url` ([configuration](../configuration.md)).

### 1.1 Account

| | |
|---|---|
| What | Handle (public name, 3 to 16 characters), e-mail address, creation time. |
| Where | `accounts.handle`, `accounts.handle_key` (lower-case copy), `accounts.email` (plain text, normalised to lower case, unique), `accounts.created`, `accounts.id`. |
| Why | Identify the player to friends, send sign-in codes, stop banned people coming back. |
| Basis (proposed) | Art. 6(1)(b). |
| Kept today | Until the player deletes the account. `DELETE /v1/me` and the account web only schedule it: the account is deleted 30 days later (`deletion_at`, `state.schedule_deletion`) and the player can cancel until then; the sweep calls `db::delete_account`. No inactivity limit: an abandoned account stays for ever. |
| Proposed | Same, plus automatic deletion after a stated period of inactivity (suggestion: 24 months, with a warning mail; lawyer to confirm). |
| Who sees it | The player sees their own e-mail. Every signed-in player can look up another account's **handle and ID** (`GET /v1/accounts/{id}`, `?handle=`), never the e-mail. **Signed-out visitors too:** the handle, the ID and whatever profile parts are set to `everyone` are public on `/u/{name}` and `/web/api/v1/users/{name}` (rate limited per IP). Moderators and the operator with database access see everything. |

Migration 0008 adds `accounts.age_confirmed_at` (when the player ticked "old enough to agree on my own"; no birth date and no age), `terms_version` and `terms_accepted_at` (the version of `terms_version` in force when the player accepted, and when). They are filled at sign-up on the web and by the API's sign-up (age only there); accounts made before are null. `tokens_valid_from` is the "not before" time that "sign out everywhere" sets. When the operator changes `terms_version`, the account web asks again at the next sign-in and overwrites the version. See G9.

### 1.2 Sign-in challenge and sign-up permit

| | |
|---|---|
| What | A pending sign-in code, and a short permit to create an account with a verified address. |
| Where | `challenges` (`id_hash`, `email` in plain text, `code_hash`, `tries`, `expires_ms`); `signups` (`id_hash`, `email` in plain text, `expires_ms`). |
| Why | Check the code the player types. |
| Basis (proposed) | Art. 6(1)(b). |
| Kept today | Code 10 minutes, 5 tries; permit 15 minutes. Expired rows are deleted by the sweep every 5 seconds (`live::sweep_expired`); a used code is deleted at once. At most 50,000 rows at a time. |
| Proposed | As today. |
| Who sees it | Nobody through the API. The operator with database access. |

### 1.3 Devices and refresh tokens

| | |
|---|---|
| What | One row per signed-in device: a name the launcher sends (up to 64 characters, free text, often a computer name), the platform (`windows`, `linux`, `macos`, `other`), a hash of the refresh token, creation and last-use times. |
| Where | `devices` (`name`, `platform`, `refresh_hash`, `created`, `last_used`). Replaced refresh tokens: `used_refresh` (`hash`, `device_id`, `used`). |
| Why | Keep the player signed in without a password; notice a stolen refresh token (a replaced token that comes back signs the device out and logs a warning). |
| Basis (proposed) | Art. 6(1)(b) for the device; Art. 6(1)(f) for the theft check (security of the service). |
| Kept today | Until sign-out, until it is replaced as the 11th device (`MAX_DEVICES` = 10; the least recently used goes), or until the account is deleted. No expiry by inactivity: `last_used` is stored but nothing reads it for that. `used_refresh` rows live 30 days (`USED_REFRESH_KEEP`), but the pruning runs only when the same device rotates its token again (`db::rotate_refresh`), so rows of a device that stops refreshing stay until the device is deleted. |
| Proposed | Expire a device not used for 90 days (suggestion; lawyer to confirm). Prune `used_refresh` in the sweep. |
| Who sees it | The player sees their device list in the export (`GET /v1/me/export`). The operator with database access. |

Since migration 0008 a device also has `device_key` (the public half of a key the player's PC made; nothing secret) and `legacy_until` (until when it may refresh without a proof). `proof_nonces` holds a hash of each accepted proof's nonce for about two minutes (`hash`, `expires_ms`), so a proof is accepted once; the sweep deletes expired rows.

### 1.3a Passkeys

| | |
|---|---|
| What | The passkeys of an account: a name the player chose (up to 40 characters), the credential ID, the credential as webauthn-rs stores it (public key, signature counter, flags), when it was added and last used. A passkey holds no biometric data: the fingerprint or face never leaves the player's device. |
| Where | `passkeys` (`id`, `account_id`, `name`, `credential`, `created`, `last_used`). Ceremonies in progress: `web_flows` (`id_hash`, `kind`, `data`, `expires_ms`). |
| Why | Sign in without a code; protect the account against phishing. |
| Basis (proposed) | Art. 6(1)(b). |
| Kept today | Until the player removes the passkey or the account is deleted (cascade). Ceremonies and one-time codes for 60 seconds to 5 minutes; the sweep deletes them. |
| Who sees it | The player on the account page and in the export (names and times, not the credentials). |

### 1.3aa Notification devices (Web Push)

| | |
|---|---|
| What | For each browser that turned on notifications: the address of its push service, the browser's public key and authentication secret, a name the player gave the device (up to 60 characters), the kinds wanted (friend request, invitation, message), when it was added and last answered, and a count of failed sends. |
| Where | `push_subscriptions` (`id`, `account_id`, `endpoint`, `p256dh`, `auth`, `label`, `kinds`, `failures`, `created_at`, `last_ok_at`). The address is the browser maker's (Google, Mozilla, Apple, Microsoft); the push service sees that a notification exists and its size, never its content (end-to-end encrypted). |
| Why | Tell the player of a friend request, an invitation or a message when the portal is closed. Only when the player turns it on, for each browser. |
| Basis (proposed) | Art. 6(1)(a), consent through the browser's permission and the switch. |
| Kept today | Until the player turns it off, the push service reports the subscription gone, five sends fail in a row, or the account is deleted (cascade). |
| Who sees it | The player on the notifications page and in the export (name, kinds, date; not the keys or the address). The operator with database access. |

### 1.3b Browser sessions and the cookie

| | |
|---|---|
| What | A session of the account web in one browser: a hash of the cookie value, the account (or none while signing in), a CSRF token, the state of a sign-in in progress (the e-mail address being verified, the loopback address of the app and the PKCE challenge, the device name the app gave) and times. The cookie (`__Host-anyplay` on https) holds a random value only. |
| Where | `web_sessions` (`id_hash`, `account_id`, `csrf`, `data`, `created_ms`, `expires_ms`). |
| Why | Keep the player signed in on the account page for a short time; carry a sign-in from one page to the next. |
| Basis (proposed) | Art. 6(1)(b). The cookie is strictly necessary for the service the player asked for, so TDDDG § 25(2) no. 2 applies and no banner is needed (lawyer to confirm). It is set when a sign-in page opens, `HttpOnly`, `SameSite=Strict`, `Secure`, without `Domain`, and ends with the browser session. |
| Kept today | 30 minutes unused, 4 hours at most; ended by signing out, signing out everywhere or deleting the account; expired rows are deleted by the sweep. |
| Who sees it | Nobody. The operator with database access sees the e-mail address of a sign-in in progress. |

### 1.4 Friends, requests and blocks

| | |
|---|---|
| What | Who is friends with whom, open requests, and who blocked whom. |
| Where | `friends` (`low`, `high`, `since`), `friend_requests` (`from_id`, `to_id`, `since`), `blocks` (`blocker`, `blocked`, `since`). |
| Why | The friends feature; blocks protect the player. |
| Basis (proposed) | Art. 6(1)(b). |
| Kept today | Until removed by a player, or until either account is deleted (`ON DELETE CASCADE`). |
| Proposed | As today. |
| Who sees it | Both sides see a friendship. Blocks are private to the blocker (README: "blocks stay private"); the blocked person learns nothing. The operator with database access. |

A block names the blocked person, so it is data about them too. The export lists blocks the player made, not blocks against them. That is likely fine under [Art. 15(4)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679) (a copy must not harm the rights of others); the lawyer should confirm.

### 1.5 Presence and activity

| | |
|---|---|
| What | Whether the account is online, which game it plays (title ID, title, version), whether that game is open to join, and when the account was last online. |
| Where | `presence` (`online`, `activity` JSON, `last_seen_ms`, `sent` JSON, `revision`). Sent to friends as `presence` events on the event connection. The account's `appear_offline` flag (`accounts`) makes the cloud send friends an offline presence instead; it is part of the account's own data (`GET /v1/me`). |
| Why | The presence feature. |
| Basis (proposed) | Art. 6(1)(b). What a player plays reveals habits, so the lawyer should confirm that no consent is needed (inference: the player starts the feature). |
| Kept today | One row per account for the life of the account; the content is overwritten. `last_seen_ms` is the time the last connection closed and stays until the next connection. |
| Proposed | As today. Keep `last_seen_ms` away from friends unless the player chooses to show it (no route exposes it today). |
| Who sees it | Friends (online state, game, joinable). It is not in the export (G6). |

### 1.6 Event connections

| | |
|---|---|
| What | A record that a device holds an open WebSocket and which node holds it. |
| Where | `connections` (`device_id`, `account_id`, `node`, `serial`, `opened_ms`). |
| Why | Route events to the right node. |
| Basis (proposed) | Art. 6(1)(b). |
| Kept today | While the connection is open. Removed on close, or by the sweep when the node's heartbeat stops (30 seconds, `node_timeout`). |
| Proposed | As today. |
| Who sees it | Nobody through the API. |

### 1.7 Sessions, members, invitations, virtual addresses

| | |
|---|---|
| What | Game sessions and their members. A member row holds the handle, the **endpoint ID** (a public key that identifies the player's tunnel endpoint), an optional relay URL, and the virtual address. A session holds the game title, version, build, a visibility and up to 8 KiB of opaque game data (`properties`, base64). Invitations hold sender, receiver, session and game. |
| Where | `sessions`, `members` (`handle`, `endpoint`, `relay`, `address`, `joined`), `invitations` (`from_id`, `from_handle`, `to_id`, `title`, `expires`). |
| Why | Let players play together. |
| Basis (proposed) | Art. 6(1)(b). |
| Kept today | A member leaves 30 seconds after its last connection closed (`session_grace`); the last member closes the session and its rows are deleted. Invitations last 15 minutes. A virtual address lives in the member's row and goes with it. The opaque `properties` are written by the game and the cloud does not look inside, so a game could put personal data there. |
| Proposed | As today. Tell game developers in the API document not to put personal data in `properties`. |
| Who sees it | Members of a session see each other's handle, endpoint ID, relay and address. **Friends of any member see the whole session, with every member's endpoint, relay and address, when its visibility is `friends`.** The endpoint ID is not an IP address. The cloud never publishes direct addresses (ARCHITECTURE, "Relays"). Sessions now also have a `kind` (`game` or `lan`, migration 0023); see section 7.4. |

The endpoint ID is a stable key. A player who keeps the net service's data folder keeps the same ID across sessions, so session members can recognise each other over time. This is by design; the lawyer should know it exists (G11).

### 1.8 Reports

| | |
|---|---|
| What | A report about another account: reporter, reported account, a reason (`cheating`, `harassment`, `spam`, `name`, `other`), free text up to 1,000 characters, a snapshot of what was reported (the reported handle and, for a reported message, its text), the time, and once decided the outcome, the time and the moderator. |
| Where | `reports` (`id`, `reporter`, `reporter_handle`, `reported`, `reported_handle`, `reason`, `details`, `content`, `created`, `status`, `outcome`, `decided`, `decided_by`, `decided_by_handle`, `action_id`). Filed with `POST /v1/reports`; at most 20 per reporter per day (`report_limit_per_day`) and 5 per 10 minutes, one open report per pair. |
| Why | Moderation of abuse; evidence for bans. |
| Basis (proposed) | Art. 6(1)(f): the operator and other players have an interest in a safe service. The free text about a third party rests on the same basis. For notices under the DSA also Art. 6(1)(c) ([dsa.md](dsa.md)). |
| Kept today | A decided report 6 months after the decision (`retention_reports_days`), an undecided one 1 year after it was filed (`retention_open_reports_days`). Deleting the **reported** account keeps the report with its handle and ID as evidence. Deleting the **reporter** blanks `reporter` and `reporter_handle` and keeps the rest. There is no foreign key from `reports` to `accounts`. |
| Proposed | As built. The lawyer should confirm the periods; the earlier suggestion was 12 months. |
| Who sees it | Moderators (`GET /v1/moderation/reports`, `anyplay-cloud admin reports`), with both handles. The reporter sees their own reports in the export. The reported person does not see reports about them. |

### 1.9 Bans

| | |
|---|---|
| What | A ban on an account and its e-mail address: account ID, a **keyed hash (HMAC-SHA-256) of the normalised e-mail address**, the end date or none, a reason (written by the moderator, up to 500 characters) and the time. The action behind it (`actions`: kind, rule, reason, moderator, time, lifting) and an appeal (`appeals`: the player's text and the decision) are separate rows. |
| Where | `bans` (`account_id`, `email_hash`, `until`, `reason`, `created`, `action_id`, `salted`); index `bans_email`. `actions`, `appeals`. Written by `moderation::act`. |
| Why | Enforce a ban, including against a new account with the same address. |
| Basis (proposed) | Art. 6(1)(f). |
| Kept today | A time-limited ban, a mute and a warning 6 months after they end or were given (`retention_bans_days`), with their appeals. A ban for good stays until a moderator lifts it, and 6 months after. The row survives account deletion on purpose. The hash is keyed with a secret that is not in the database (`ban_secret` or `data_dir/ban.key`); hashes made before it (plain SHA-256) are replaced when their account still exists or when the address shows up, and match until then. The banned person reads the reason ([dsa.md](dsa.md), Art. 17). |
| Proposed | A review period for bans for good (suggestion: 5 years) is not built; a moderator lifts them. |
| Who sees it | Moderators and the operator. The banned person sees the statement of reasons (not the moderator's name). The operator with database access. |

## 2. Memory only

Not written to disk by the cloud.

### 2.1 Rate limit counters

| | |
|---|---|
| What | Counters whose keys include the **client IP address** (`email:ip:{ip}`, `code:ip:{ip}`, `admin:{ip}`), a **hash of the e-mail address** (`email:address:{sha256 hex}`), the account ID (`account:{id}`) and the device ID (`refresh:{id}`). |
| Where | `RateLimiter.windows` in `crates/cloud/src/limits.rs`. The client IP comes from the socket, or from the first `X-Forwarded-For` hop when `trust_proxy` is on (`http.rs`, `ClientIp`). |
| Why | Stop abuse and guessing of sign-in codes. |
| Basis (proposed) | Art. 6(1)(f): security of the service. An IP address is an online identifier, so personal data under [Art. 4(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679). |
| Kept today | Per window: 15 minutes for sign-in routes, 1 hour for refresh, 60 seconds for account and admin routes. The sweep drops finished windows every 5 seconds. Lost on restart. Each node holds its own copy. |
| Proposed | As today. It is the shortest retention that works. |
| Who sees it | Nobody. |

### 2.2 Caches

`AuthCache` (device to account, account to ban) is rebuilt from the database and holds no more than the tables above.

## 3. Logs and metrics

### 3.1 Cloud log

Written to standard output by `tracing`. The default filter is `info,sqlx=warn` (`main.rs`); `RUST_LOG` changes it. The format is `text` or `json` (`log_format`). The code sets **no retention**; whatever collects stdout decides (see 3.4).

| Line | Fields | Level | Personal data |
|---|---|---|---|
| `sign-in code (mail = log)` | `to` (e-mail), `code` | info | **E-mail address and a live code.** Only with `mail = log`, which is the **default** in `config.rs`; `deploy/compose.yaml` defaults to `smtp`. For development only. |
| `moderation action` | `account` ID, kind | warn | Account ID. No actor. The record is the `audit_log` table, not this line. |
| `notice (mail = log)` | `to`, subject | info | **E-mail address**, with `mail = log` only. |
| refresh-token events (replaced, used again) and passkey errors | `device` ID, passkey error text | info/warn | Device ID. Whether the passkey error text can carry values is **not checked**. |
| `a replaced refresh token was used again` | `device` ID | warn | Device ID. |
| `request` (method, path, status, ms) | `path` can hold account, session or invitation IDs; the query string and the body are left out | **debug** (off by default) | IDs in the path. |
| `no pong; dropping the connection` | `device` | debug | Device ID. |
| database and mail errors | error text | error | Mail replies have every word containing `@` replaced (`mail.rs`). Database error text from sqlx is **not checked** for values. |
| `cannot send a sign-in code` | none | error | None. |

Not logged: client IP addresses, e-mail addresses (except with `mail = log`), code values (except with `mail = log`), tokens, request bodies. The cloud writes no access log of its own.

### 3.2 Metrics

`GET /metrics` on `metrics_listen` (off by default). Counters per route template and status code (`anyplay_cloud_http_requests_total{route,status}`), request time, event connections, news, sweeps, store errors and the node name. No account, IP or handle labels. No personal data.

### 3.3 Reverse proxy (Caddy)

`deploy/Caddyfile` has no `log` directive. Our reading of Caddy's documentation is that HTTP access logs are then off and only Caddy's own runtime log is written, but we did not run it to confirm (open item in [open-questions.md](open-questions.md)). If an operator turns access logs on, they hold the client IP address, the URL path (account IDs; handle lookups are in the query string, so handles too) and the user agent, and need their own retention.

The proxy sets `X-Forwarded-For`, which the cloud trusts (`ANYPLAY_CLOUD_TRUST_PROXY: "true"`). Caddy keeps its ACME account data and certificates in the `caddy-data` volume. `ACME_EMAIL` goes to the certificate authority; it is the operator's address, not a player's.

### 3.4 Docker log driver

`deploy/compose.yaml` sets the `json-file` driver with 3 files of 10 MB per container: a size limit, not a time limit. Lines are overwritten when the files are full.

## 4. The relay

Code: `crates/relay`. It is `iroh-relay` plus ticket checks (`crates/relay/src/lib.rs`).

| | |
|---|---|
| What the relay handles | The **IP address and port** of every connected client (it is the other end of the connection), the client's endpoint ID, and the relay ticket. The ticket is a signed token that names the **account ID**, the endpoint ID and an expiry (`Claims::relay`; lifetime 24 hours, `relay_ttl_secs`). Game and voice traffic passes through it end-to-end encrypted; the relay cannot read it. With QUIC address discovery on UDP 7842 the relay tells a client its own public address; it does not need to store it. |
| Linkable? | In principle the relay can join an IP address to an account ID, because the ticket carries the account ID and the relay sees the connection's address in the same request. It never calls the cloud. We found no code that stores the pair. |
| What our code logs | Start-up lines (ports, TLS mode). No per-connection line. The relay keeps a connection count per account (from the ticket) and each client's IP and port in memory while connected (`limits.rs`). |
| What `iroh-relay` logs or counts | **Not verified.** We read the log lines in `iroh-relay` 1.4.1 that name an address and found only listen addresses. We did not audit trace-level output or its metrics. Open item. |
| Rate limit | Optional bytes per second per client (`client_bytes_per_second`), held in memory by the library. |
| Kept today | Nothing on disk by our code. Connection state in memory while connected. |
| Proposed | Say in the operator guide that a relay must not log IP addresses, or keep such logs no longer than 7 days (suggestion) when needed to stop abuse. Basis for any such log: Art. 6(1)(f). |
| Who sees it | The relay operator. |

A player who uses the relay-only setting sends all game traffic through the relay, so their direct IP address reaches only the relay and not the other players ([network ADR 0002 and DESIGN](https://github.com/anyplaynet/network/blob/main/docs/DESIGN.md)).

## 5. Hosting, backups and processors

| Item | State in the repository | Gap |
|---|---|---|
| Database file or Postgres | `cloud-data` volume (compose) or the operator's Postgres. | `deploy/backup.sh` saves the volume before every update and keeps the newest 14 archives (not encrypted, mode 0600); an operator may add more (see G10). A deleted account stays in backups until they expire. |
| Signing key | `signing.key` in `data_dir`, public half `signing.pub`. | Not personal data. Loss or theft lets someone forge tickets; a matter for security under [Art. 32(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679). |
| Mail | The SMTP server named by `smtp_host` receives the recipient address and the code of every sign-in mail. | A processor under [Art. 28](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): a contract is needed. |
| Hosting | The operator's server or cloud account holds everything above. | A processor contract is needed if it is a third party. Location in the EU (research note). |
| Certificate authority | Let's Encrypt via Caddy. | Receives the operator's contact address and host names only. |
| Update and data checks | The launcher fetches release data from `api.github.com` and settings from `raw.githubusercontent.com` (`toolkit.rs`, `recommended.rs`, `community.rs`). GitHub sees the player's IP address. | Not the cloud's, but the launcher's privacy notice must say it. |

## 6. The player's PC (launcher and net service)

The cloud does not control this data, but the cloud's privacy notice must say what the apps send to it.

| What | Where | Sent to the cloud? |
|---|---|---|
| Sign-in: device ID and refresh token | The OS credential store (service `AnyPlay`), or, when there is none, the file `signin-<cloud>.json` in the net service data folder (`network/crates/net/src/credentials.rs`; "a file that only its owner may read") | The refresh token, on `POST /v1/auth/refresh`. |
| Endpoint secret key | A file in the net service data folder | Only the public endpoint ID, in session joins and ticket requests. |
| Device name, platform | Read from the PC, sent at sign-in | Yes: `devices.name`, `devices.platform`. |
| Activity | Title ID, title, version of the running game | Yes, in presence; friends see it. |
| Session data | Endpoint ID, relay URL | Yes (see 1.7). |
| HTTP details | `User-Agent: anyplay-net/<version>`; source IP address | Yes, on every request. The cloud uses the IP for rate limits only. |
| Observation logs | `observations/<title ID>.jsonl` in the data folder; bridge logs when the player turns on *Record Network Calls* | **No.** "An observation log never leaves the player's PC on its own" ([observation log spec](https://github.com/anyplaynet/network/blob/main/spec/observation-log.md)). They can hold what a game sends. |
| Launcher preferences | `preferences.json` in the user's config folder (`launcher/crates/anyps5-core/src/paths.rs`) | No. |
| Game files, saves | The player's disk | No. Never read by the cloud. |

Whether the launcher or net service stores or reads anything on the device that needs consent under § 25 TDDDG is an open question ([open-questions.md](open-questions.md)).

## 7. The social layer

Built and live (migrations 0003 to 0023). Audited against the code at cloud `8e0e02d` on 25 October 2026. Feature by feature: what is kept, who sees it, how long, and what account deletion does. Counts and periods are the code defaults; the operator can change several in `config.rs`.

### 7.1 Profile, avatar, trophies, playtime, run reports

| Data | Where | Who sees it | Kept | On deletion | Export |
|---|---|---|---|---|---|
| About (160 characters), up to 3 favourite titles | `profiles` (0006) | By the `privacy` setting per part: `friends` (default), `everyone` (**also signed-out visitors**, on `/u/{name}` and `/web/api/v1/users/{name}`; the about text also goes into the page description, at most 200 characters) or `nobody`. A block hides everything. | Life of the account | Deleted (cascade) | Yes |
| Avatar | `avatars` (PNG, 256 by 256, metadata dropped), SHA-256 `hash` | Same rule. **`GET /v1/avatars/{hash}` needs no token** and is cached publicly for a year (`immutable`): whoever has the hash can fetch it, and caches can keep it after removal. | Until removed or replaced | Deleted | Yes (base64) |
| Playtime | `playtime` (title, day, minutes; at most 20,000 rows) | `privacy.playtime` | Life of the account, no sweep | Deleted | Yes |
| Trophies | `trophies` (title, number, grade, time), `trophy_secrets` (0021) | `privacy.trophies`; secret trophies hidden until revealed. Compare shows one player's trophies next to another's when allowed. **Anonymous rarity counts every player's records once 10 players have the title, whatever the setting.** | Life of the account | Deleted | Yes |
| Level and points | Derived from trophies, not stored | Wherever trophies are visible | | | |
| Run reports | `run_reports` (verdict, OS, game version, toolkit; at most 1,000 rows) (0016) | Public only as counts, and only with at least 3 reports (`/compatibility`) | Life of the account | Deleted | Yes |
| Privacy settings | `privacy` | The player | Life of the account | Deleted | Yes |
| Linked accounts | `linked_accounts` (provider, external ID, display name, avatar, audience, time) (0013) | Per `audience`, signed-out included; the name and picture are a snapshot from linking | Until unlinked | Deleted | Yes, without the avatar |

Discord tokens are not stored (`discord_sdk.rs`); login tokens from linking are revoked at once. Steam linking exists in the cloud and is hidden on the web.

### 7.2 Messages

| What | Detail |
|---|---|
| Tables | `conversations` (kind, group name, party, pair, last sequence), `conversation_members` (state, read mark), `messages` (`body`, sender, time), `message_hidden` (0007) |
| Encryption | **None at application level.** Stored as plain text; only the transport is TLS. The operator can read everything through the database. |
| Who sees it | Members, from the time they joined, minus messages hidden by a block. Read marks are shown to the other members. Moderators see a message only as the snapshot inside a report. |
| Kept | Each message 30 days from sending (`message_retention_days`). An empty direct or group conversation 30 days after its last activity. A party chat ends with the party. |
| Limits | 5 unanswered messages to a stranger; an account younger than 24 hours cannot write first to a stranger; a muted account cannot write. |
| On deletion | The player's own messages are deleted. **A direct conversation is deleted whole, with the other person's messages.** Group conversations keep what others wrote. |
| Export | The player's own messages and the conversations; **not** what others wrote to the player. |
| Not built | Attachments other than the screenshots and clips of section 7.2b. Reactions, edit and unsend are in an open pull request (cloud#74) and must be added here when merged. |

### 7.2a Looking for group listings

| What | Detail |
|---|---|
| Tables | `lfg_listings` (poster, game title ID, mode, language, a note of at most 140 characters without links, slots, times, closed time) and `lfg_requests` (listing, account that asked, times, accepted time), migration 0025 |
| Purpose | A player says they look for others to play a game with; others who have the game ask to join; the poster accepts them into their party. |
| Basis (proposed) | Art. 6(1)(b), the feature the player chose to use; the note is what the player chose to write. |
| Who sees it | Every signed-in player who asks for the board of that game and is not blocked either way with the poster: handle, account ID, game, mode, language, note, places. The cloud cannot tell who owns the game; that is the launcher's filter, not a rule. The poster sees who asked. A player who asked sees their own request. |
| Kept | A listing lives 1 hour. Closed, full or expired listings are deleted a day later with their requests. A ban closes the account's listings at once. |
| On deletion | Both tables cascade with the account. |
| Export | The player's own listings and requests (`looking_for_group`). |
| Reports | A report of a listing saves the note in the report (section 1.8). |

### 7.2b Screenshots and clips sent to a friend

| What | Detail |
|---|---|
| Tables and files | `media` (file ID, conversation and message number, sender, kind, type, size, picture size, length, times, removed time, hold time; migration 0026) and the file itself in `media_dir` under a random 128-bit key. |
| Purpose | A player sends a screenshot or clip of their game to one friend, who sees it in the conversation. |
| Basis (proposed) | Art. 6(1)(b) for the feature the player used. |
| Content | Whatever the player chose to send. The cloud checks the file type, size and (for a clip) length. **It does not look at the picture** and does not remove metadata; a screenshot made by the launcher has none. |
| Who sees it | The two friends only: a direct conversation both accepted, between friends, with no block. Never strangers, message requests or groups. Access is checked on every read; the key alone gives nothing. Moderators see a file only when a report holds it. |
| Kept | 30 days from sending, then deleted from disk and from the table. Unsent by the sender: out of reach at once, kept 7 days for a report, then deleted. A report holds the file as long as an undecided report lives (`retention_open_reports_days`, 365). **Held files survive account deletion** like the report. |
| On deletion | The account's files that no report holds are deleted from disk. When one friend deletes their account the direct conversation goes, and the other's files in it become unreachable and are deleted at their 30 days. |
| Export | The list of what the player sent (`media_sent`: ID, kind, time), not the files. |
| Backups | The media folder is part of the volume backup (section 5) and so is kept as long as the backup. |
| Limits | 20 uploads per hour and 1 GiB of live files per account; four uploads at a time per node. |

**Privacy page:** the privacy policy on the website must name both features (LFG listings visible to other players for an hour; screenshots and clips visible to the chosen friend for 30 days and kept for moderation when reported). OPERATOR AGENT: update the legal privacy page for LFG listings and shared screenshots/clips.

### 7.3 Feed

Tables `feed_items`, `feed_members`, `feed_reactions`, `feed_firsts` (0017). Item kinds: a trophy, a new friendship (naming the friend), a first-played title, a party. Visible to the owner and the owner's friends unless `privacy.feed` is `nobody` (then nothing is recorded); party items to the party's members. Reactions show as counts; nobody sees who reacted. Items are kept 90 days (`feed_retention_days`), at most 100 a day per account. `feed_firsts` (every title the account was seen playing, at most 2,000) is kept for the life of the account with no sweep. Deletion cascades; a party item owned by someone else keeps the deleted account's ID in its `members` text until the 90-day sweep. The export holds the player's own items and reactions, not `feed_firsts`.

### 7.4 Parties, calls, voice, sessions, join codes

| Data | Where | Who sees it | Kept |
|---|---|---|---|
| Party and members (handle, endpoint, relay, voice flag, optional Discord link set by the leader) | `parties`, `party_members` (0007, 0009, 0019) | Members | A member leaves 15 minutes after disconnecting; at most 8 members |
| Calls | A call is a party invitation with `call = 1` that rings 30 seconds | The invited player | Not kept |
| Missed calls | `missed_calls` (caller, handle, time) | The called player | 30 days; also cleared when seen, on unfriend or block. **Not in the export.** |
| Sessions and members (handle, endpoint, relay, virtual address, game properties up to 8 KiB, kind `game` or `lan`) | `sessions`, `members` (0002, 0023) | Members and invitees; **friends of any member when visibility is `friends`**, including every member's endpoint, relay and virtual address | A member leaves 30 seconds after the last connection; a session ends with its last member (a `lan` session with its host); invitations 15 minutes |
| Join codes | `join_codes` (SHA-256 of the code, expiry), one per account (0018) | Anyone with a valid code sees, without signing in, the issuer's handle, the title, the member count and the kind | 120 minutes or until revoked. **Not in the export.** |

**Voice audio never reaches the cloud.** It runs over QUIC on the iroh tunnel between the net services, directly or through the AnyPlay relay, end-to-end encrypted. The cloud stores only the voice flag, endpoint and relay. There are no recordings and no call log beyond missed calls. The endpoint key and the preferred virtual address (derived from the account ID) stay the same across sessions.

### 7.5 Presence and "Appear offline"

`presence` holds online, activity (title ID, title, version), `last_seen_ms` (never sent to anyone) and a revision; one row per account, overwritten; the activity is cleared when going offline. `accounts.appear_offline` (0020) makes friends see the account as offline and stops "first played" items. Presence is not in the export.

### 7.6 Notifications (Web Push)

`push_subscriptions` (endpoint, `p256dh`, `auth`, label, kinds, failures; at most 10 per account) (0015). The payload carries a title and the sender's handle, never message text, and goes through the browser maker's push service (Google, Mozilla, Apple, Microsoft). Removed when turned off, on 404 or 410, or after 5 failed sends in a row. The export omits endpoint and keys. The browser stores `localStorage["anyplay.push.device"]` and a service worker (`/sw.js`).

### 7.7 Moderation records, in short

Reports, actions, appeals, bans and the audit log are described in [moderation.md](../moderation.md). Retention: decided reports 183 days after the decision, open ones 365 days; actions and appeals 183 days after they end (a ban for good with no end is never swept); temporary bans 183 days after the end; the audit log 1,095 days. **All of these survive account deletion.** The appeal text is copied into the audit log, so it outlives the appeal.

### 7.8 Account deletion and export, as built

Deletion has a 30-day grace period (`deletion_at`). `DELETE /v1/me` and the web both schedule it, sign out every device, mail a notice, and can be cancelled. **For those 30 days the account stays visible and usable**; nothing hides it. The sweep then deletes the account and cascades; no record of the deletion is kept. Left behind: the moderation records above, party feed items owned by others (up to 90 days), relay tickets (up to 24 hours), cached avatars (up to a year), log lines, and backups (see section 5).

The export (`GET /v1/me/export`, `/web/api/v1/account/export`) covers the account, devices, friends, requests, own blocks, reports filed, the 500 newest actions against the player, ban (without the hash), sessions, trophies, run reports, privacy, linked accounts, notification devices, passkeys (name and dates), consent, profile, avatar, playtime, own feed items, party, conversations and own messages. It lacks presence, `feed_firsts`, missed calls, invitations, the join code, web sessions, messages received from others and records others hold about the player (see G6).

## Gaps

Ordered by how much they matter. The IDs are used in the other documents.

| ID | Gap | Where | Suggested change |
|---|---|---|---|
| G1 | **Closed** (moderation change). Bans are swept 6 months after they end; the e-mail hash is keyed; the reason is shown. Open: older hashes of deleted accounts stay plain until they match or expire; bans for good have no review period. | `bans`, `moderation::banhash` | |
| G2 | **Closed** (moderation change). Reports have a status, outcome and handler, keep a snapshot, survive both accounts and are swept. | `reports` | |
| G3 | **No inactivity deletion.** Accounts and devices live until the player acts. `used_refresh` rows are only pruned when the same device refreshes again, and `feed_firsts` and `playtime` have no sweep. | `accounts`, `devices` | Inactivity expiry with a warning mail. |
| G4 | **The default `mail = log` writes e-mail addresses and live codes to the log.** | `config.rs`, `mail.rs` (`LogMailer`) | Refuse to start in log mode when `signup = open` and the listen address is not loopback; or make `smtp` the default. |
| G5 | **IP addresses reach three places that need a stated rule:** the rate limiter in memory (short, fine), the relay (logging unverified; it sees the IP and a ticket with the account ID), and the proxy if access logs are on. No log retention is set anywhere. | `limits.rs`, relay, `Caddyfile`, `compose.yaml` | Compose `logging:` limits; documented relay and proxy log settings; check `iroh-relay` output. |
| G6 | **The export is incomplete.** It lacks presence and last-seen, `feed_firsts`, missed calls, invitations, the join code, web sessions, messages received from others and the records others hold about the player, and the ban hash. It is JSON, which fits the "structured, commonly used and machine-readable" format of [Art. 20(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679). | `routes/accounts.rs` (`export`) | Add presence and device `last_used`; document what is left out and why. |
| G7 | **Partly closed.** Deletion now has a 30-day grace period, a mailed notice and a confirmation step on the web (fresh session and the typed handle); `DELETE /v1/me` still needs only an access token. During the grace period the account stays visible. It leaves bans, reports, actions, appeals and the audit log, and nothing covers backups. No record that a deletion happened is kept. | `state.delete_account` | Confirm by e-mail code; document backup expiry; keep a non-identifying deletion counter. |
| G8 | **Closed** for moderation (moderation change). Every action, dismissal, lifting, appeal and role change is a line in `audit_log` with the actor. Moderators are accounts; the admin token acts as `operator`. | `moderation` | |
| G9 | **Age and terms record: done in migration 0008.** `age_confirmed_at`, `terms_version` and `terms_accepted_at` are stored on the account (no birth date); the account web names the age (16 in the EU unless the country sets a lower one) and asks again when `terms_version` changes. Accounts made before the migration have no record; the API's own sign-up records the age only. | `routes/auth.rs` (`create_account`), `web/flow.rs` | Decide whether older accounts are asked once (O-question for the lawyer); the privacy notice should name the cookie and the passkeys. |
| G10 | **Partly closed** (backup change). `deploy/backup.sh` archives the whole `cloud-data` volume (database, `signing.key`, `ban.key`) before every update and keeps the newest 14 (mode 0600, SHA256 manifest); `deploy/restore.sh` proves an archive in a scratch volume (integrity check, account count) and is the documented restore path ([deploy.md](../deploy.md#backups)). **Open:** no encryption of the archives at rest, no off-machine copy, an erasure period for backups is not decided (a restore brings back accounts deleted since; deletions must be replayed), and processor contracts for a backup provider. | `deploy/backup.sh`, `deploy/restore.sh`, `docs/deploy.md` | Decide the retention period and the off-machine target; see [open-questions.md](open-questions.md). |
| G11 | **A stable endpoint ID and handle in sessions** let members recognise each other across sessions. | `members` | Document it; consider a per-session endpoint pseudonym later. |
