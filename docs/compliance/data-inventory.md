# Data inventory

What a cloud and its relay actually store or log about people, read from the code at the commit this pull request is based on. This is a draft for legal review, not legal advice ([README](README.md)).

How to read it:

- **Where** names the table and column, the log line or the metric.
- **Basis (proposed)** is our suggestion for the legal basis under [GDPR Art. 6(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): (b) the processing is necessary for a contract with the person, (c) a legal obligation, (f) legitimate interests, unless the person's interests override them (Art. 6(1)(f) names children as a case that weighs heavily). Every basis here is an inference for the lawyer to confirm.
- **Kept today** is what the code does. **Proposed** is what we think it should do. Where they differ, the difference is a gap in [Gaps](#gaps). Storage limitation is the rule in [Art. 5(1)(e)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679): identifiable data "no longer than is necessary".
- "Operator" means whoever runs the cloud. "Moderator" means a person holding the admin token (`admin_token`); the code has no other staff role.
- Account, device and session IDs are random numbers (`util::random_id`). They are pseudonyms, not anonymous: the cloud can join them to the e-mail address ([Art. 4(1) and 4(5)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679)).

Trophy records, profiles, messages and the feed are not in the code yet. Migration 0003 (trophy records, privacy settings) is in flight in another change, per [network ADR 0009](https://github.com/anyplay-gg/network/blob/main/docs/adr/0009-trophy-records-not-trophy-texts.md). That ADR says the cloud keeps only title ID, trophy number, grade and time. [ADR 0011](https://github.com/anyplay-gg/network/blob/main/docs/adr/0011-messages-are-stored-to-be-delivered.md) says messages are stored readable by the cloud for a limited time, and [ADR 0012](https://github.com/anyplay-gg/network/blob/main/docs/adr/0012-private-by-default.md) says profiles start private. The section [In flight](#in-flight-social-layer) lists what to add to this file when they land. (These ADRs are on the network branch `feat/trophies`, not yet on its `main`.)

## 1. The cloud database

Migrations: `crates/cloud/migrations/{sqlite,postgres}/0001_init.sql` and `0002_shared_state.sql`. A SQLite file `anyplay-cloud.db` in `data_dir`, or Postgres via `database_url` ([configuration](../configuration.md)).

### 1.1 Account

| | |
|---|---|
| What | Handle (public name, 3 to 16 characters), e-mail address, creation time. |
| Where | `accounts.handle`, `accounts.handle_key` (lower-case copy), `accounts.email` (plain text, normalised to lower case, unique), `accounts.created`, `accounts.id`. |
| Why | Identify the player to friends, send sign-in codes, stop banned people coming back. |
| Basis (proposed) | Art. 6(1)(b). |
| Kept today | Until the player deletes the account (`DELETE /v1/me` calls `state.delete_account`, then `db::delete_account`). No inactivity limit: an abandoned account stays for ever. |
| Proposed | Same, plus automatic deletion after a stated period of inactivity (suggestion: 24 months, with a warning mail; lawyer to confirm). |
| Who sees it | The player sees their own e-mail. Every signed-in player can look up another account's **handle and ID** (`GET /v1/accounts/{id}`, `?handle=`), never the e-mail. Moderators and the operator with database access see everything. |

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
| Where | `presence` (`online`, `activity` JSON, `last_seen_ms`, `sent` JSON, `revision`). Sent to friends as `presence` events on the event connection. |
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
| Who sees it | Members of a session see each other's handle, endpoint ID, relay and address. The endpoint ID is not an IP address. The cloud never publishes direct addresses (ARCHITECTURE, "Relays"). |

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
| `account banned` / `account unbanned` | `account` ID, end date | warn | Account ID. No actor. The record is the `audit_log` table, not this line. |
| `a replaced refresh token was used again` | `device` ID | warn | Device ID. |
| `request` (method, path, status, ms) | `path` can hold account, session or invitation IDs; the query string and the body are left out | **debug** (off by default) | IDs in the path. |
| `no pong; dropping the connection` | `device` | debug | Device ID. |
| database and mail errors | error text | error | Error text from sqlx or the SMTP library. We did not check whether any error text can echo an e-mail address (open item). |
| `cannot send a sign-in code` | none | error | None. |

Not logged: client IP addresses, e-mail addresses (except with `mail = log`), code values (except with `mail = log`), tokens, request bodies. The cloud writes no access log of its own.

### 3.2 Metrics

`GET /metrics` on `metrics_listen` (off by default). Counters per route template and status code (`anyplay_cloud_http_requests_total{route,status}`), request time, event connections, news, sweeps, store errors and the node name. No account, IP or handle labels. No personal data.

### 3.3 Reverse proxy (Caddy)

`deploy/Caddyfile` has no `log` directive. Our reading of Caddy's documentation is that HTTP access logs are then off and only Caddy's own runtime log is written, but we did not run it to confirm (open item in [open-questions.md](open-questions.md)). If an operator turns access logs on, they hold the client IP address, the URL path (account IDs; handle lookups are in the query string, so handles too) and the user agent, and need their own retention.

The proxy sets `X-Forwarded-For`, which the cloud trusts (`ANYPLAY_CLOUD_TRUST_PROXY: "true"`). Caddy keeps its ACME account data and certificates in the `caddy-data` volume. `ACME_EMAIL` goes to the certificate authority; it is the operator's address, not a player's.

### 3.4 Docker log driver

`deploy/compose.yaml` sets no `logging:` options. We believe Docker's default `json-file` driver does not rotate (inference from Docker's documented default; not checked here), so the stdout of all services can grow without limit on the host. Set a size and file-count limit.

## 4. The relay

Code: `crates/relay`. It is `iroh-relay` plus ticket checks (`crates/relay/src/lib.rs`).

| | |
|---|---|
| What the relay handles | The **IP address and port** of every connected client (it is the other end of the connection), the client's endpoint ID, and the relay ticket. The ticket is a signed token that names the **account ID**, the endpoint ID and an expiry (`Claims::relay`; lifetime 24 hours, `relay_ttl_secs`). Game and voice traffic passes through it end-to-end encrypted; the relay cannot read it. With QUIC address discovery on UDP 7842 the relay tells a client its own public address; it does not need to store it. |
| Linkable? | In principle the relay can join an IP address to an account ID, because the ticket carries the account ID and the relay sees the connection's address in the same request. It never calls the cloud. We found no code that stores the pair. |
| What our code logs | `relay client refused` with the endpoint ID and the reason, at debug level only (off by default). Start-up lines (ports, TLS mode). |
| What `iroh-relay` logs or counts | **Not verified.** We read the log lines in `iroh-relay` 1.4.1 that name an address and found only listen addresses. We did not audit trace-level output or its metrics. Open item. |
| Rate limit | Optional bytes per second per client (`client_bytes_per_second`), held in memory by the library. |
| Kept today | Nothing on disk by our code. Connection state in memory while connected. |
| Proposed | Say in the operator guide that a relay must not log IP addresses, or keep such logs no longer than 7 days (suggestion) when needed to stop abuse. Basis for any such log: Art. 6(1)(f). |
| Who sees it | The relay operator. |

A player who uses the relay-only setting sends all game traffic through the relay, so their direct IP address reaches only the relay and not the other players ([network ADR 0002 and DESIGN](https://github.com/anyplay-gg/network/blob/main/docs/DESIGN.md)).

## 5. Hosting, backups and processors

| Item | State in the repository | Gap |
|---|---|---|
| Database file or Postgres | `cloud-data` volume (compose) or the operator's Postgres. | No backup, restore or retention is defined. The configuration says only "Back this folder up". A deleted account stays in backups until they expire. |
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
| Observation logs | `observations/<title ID>.jsonl` in the data folder; bridge logs when the player turns on *Record Network Calls* | **No.** "An observation log never leaves the player's PC on its own" ([observation log spec](https://github.com/anyplay-gg/network/blob/main/spec/observation-log.md)). They can hold what a game sends. |
| Launcher preferences | `preferences.json` in the user's config folder (`launcher/crates/anyps5-core/src/paths.rs`) | No. |
| Game files, saves | The player's disk | No. Never read by the cloud. |

Whether the launcher or net service stores or reads anything on the device that needs consent under § 25 TDDDG is an open question ([open-questions.md](open-questions.md)).

## In flight: social layer

Not in the code at this commit. When these land, add rows here.

| Feature | What the design says | Add to this file |
|---|---|---|
| Trophy records, playtime (migration 0003) | Title ID, trophy number, grade and time; playtime as minutes per day. Private by default; friends only unless the player opens it. Rarity is counted across players. | Table and columns; retention (suggestion: life of the account); the visibility column; rarity counts must not reveal single players; include in export and deletion. |
| Profile, avatar | Handle, picture upload, short text, favourite games. | Picture storage, checks and size; whether pictures can reveal special categories (inference: not intended). |
| Messages | Stored readable by the cloud to deliver to every device, kept "for a limited time" (ADR 0011 names no period), also used for reports. | Table, retention, who may read (moderators only on a report), report attachment copy. This is the highest-risk new data ([open-questions.md](open-questions.md)). |
| Feed | Kept 30 days (network `docs/SOCIAL.md`). | Table, 30-day sweep. |
| Parties and voice | Voice goes through the tunnels, not the cloud. | Party membership table only. |

## Gaps

Ordered by how much they matter. The IDs are used in the other documents.

| ID | Gap | Where | Suggested change |
|---|---|---|---|
| G1 | **Closed** (moderation change). Bans are swept 6 months after they end; the e-mail hash is keyed; the reason is shown. Open: older hashes of deleted accounts stay plain until they match or expire; bans for good have no review period. | `bans`, `moderation::banhash` | |
| G2 | **Closed** (moderation change). Reports have a status, outcome and handler, keep a snapshot, survive both accounts and are swept. | `reports` | |
| G3 | **No inactivity deletion.** Accounts and devices live until the player acts. | `accounts`, `devices` | Inactivity expiry with a warning mail. |
| G4 | **The default `mail = log` writes e-mail addresses and live codes to the log.** | `config.rs`, `mail.rs` (`LogMailer`) | Refuse to start in log mode when `signup = open` and the listen address is not loopback; or make `smtp` the default. |
| G5 | **IP addresses reach three places that need a stated rule:** the rate limiter in memory (short, fine), the relay (logging unverified; it sees the IP and a ticket with the account ID), and the proxy if access logs are on. No log retention is set anywhere. | `limits.rs`, relay, `Caddyfile`, `compose.yaml` | Compose `logging:` limits; documented relay and proxy log settings; check `iroh-relay` output. |
| G6 | **The export is incomplete.** It lacks presence and last-seen, the ban hash. It is JSON, which fits the "structured, commonly used and machine-readable" format of [Art. 20(1)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32016R0679). | `routes/accounts.rs` (`export`) | Add presence and device `last_used`; document what is left out and why. |
| G7 | **Deletion is immediate and complete for live data, but leaves bans, and nothing covers backups.** There is no confirmation step and no record that a deletion happened, so a stolen access token (15 minutes) can delete an account. | `state.delete_account` | Confirm by e-mail code; document backup expiry; keep a non-identifying deletion counter. |
| G8 | **Closed** for moderation (moderation change). Every action, dismissal, lifting, appeal and role change is a line in `audit_log` with the actor. Moderators are accounts; the admin token acts as `operator`. | `moderation` | |
| G9 | **Age and terms record: done in migration 0008.** `age_confirmed_at`, `terms_version` and `terms_accepted_at` are stored on the account (no birth date); the account web names the age (16 in the EU unless the country sets a lower one) and asks again when `terms_version` changes. Accounts made before the migration have no record; the API's own sign-up records the age only. | `routes/auth.rs` (`create_account`), `web/flow.rs` | Decide whether older accounts are asked once (O-question for the lawyer); the privacy notice should name the cookie and the passkeys. |
| G10 | **Backups, restore tests and processor contracts are undefined** in the repository. | `docs/configuration.md` | See [open-questions.md](open-questions.md). |
| G11 | **A stable endpoint ID and handle in sessions** let members recognise each other across sessions. | `members` | Document it; consider a per-session endpoint pseudonym later. |
