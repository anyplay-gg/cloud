# AnyPlay Cloud

The services behind the [AnyPlay network](https://github.com/anyplay-gg/network): accounts, friends, presence, sessions, invitations and moderation, plus relays that carry game traffic when two players cannot reach each other directly. One program runs a whole cloud on a home server with `docker compose up`. The same program is being built to run as a cluster that carries a network of millions of players.

**Status: version 0.** Everything in the [cloud API](https://github.com/anyplay-gg/network/blob/main/spec/cloud-api.md) works and is tested end to end. No public cloud runs yet, and the API may still change until one does. The road from one process to a cluster is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## What it does

| Service | What players get | State |
|---|---|---|
| Accounts | Sign-in without a password: a code by e-mail, then a refresh token per device that changes on every use. Export and deletion of all data. | works |
| Friends and blocks | Requests by handle, mutual requests become friends at once, blocks stay private. | works |
| Presence | Who is online, what they play, and whether their game is open to join. | works |
| Sessions | Create, join and leave, with capacity, a leader, opaque game properties and a version guard: only the same game version and AnyPS5 build play together. | works |
| Invitations | Invite friends into a session; the invitation ends with the session. | works |
| Trophies | Records of earned trophies (title, number, grade, time; no names or art), rarity once 10 players own a title, gamer cards with counts by grade. Private by default: friends see them, and each player can show them to everyone or nobody. | works |
| Profiles | A short text (160 characters, no links), up to 3 favourite titles, an avatar and playtime per title, each with its own visibility (friends by default, everyone or nobody). Avatars are PNG or JPEG up to 1 MB, decoded with limits against decompression bombs, cropped square, scaled to 256 by 256 and stored again as PNG without metadata, in the database, served by content hash with long cache headers; 5 uploads per account and hour. Playtime keeps the highest minutes seen per title and day, so uploads can repeat. | works |
| Relays | Signed tickets that let one endpoint use one relay for 24 hours. Relays check them offline and never call the cloud. | works |
| Moderation | Reports, bans per account and e-mail address, rate limits on every route, and an admin command line. | works |
| Matchmaking, leaderboards, save backups, chat | | planned |

## How it is built

```text
 net service (one per player)                              relays
   │  HTTPS: requests          WebSocket: events             │ iroh-relay + ticket check,
   ▼                                  ▲                      │ end-to-end encrypted traffic
 ┌────────────────────────────────────┴───┐                  │
 │ anyplay-cloud  (Rust: tokio, axum)     │── signs tickets ─┘
 │  auth · friends · presence · sessions  │
 │  invitations · moderation · news       │
 └───────────────┬────────────────────────┘
                 │  sqlx
       SQLite (one process) · PostgreSQL (cluster, planned)
```

- **Requests and events.** The net service calls a small JSON API over HTTPS and holds one WebSocket that pushes events: a friend's presence, an invitation, a change in a session. Events are short, and the client reads the full state after each reconnect, so a lost event never leaves it wrong.
- **Tokens.** The cloud signs its own tokens with Ed25519 ([`crates/ticket`](crates/ticket)): access tokens that live 15 minutes, and relay tickets bound to one endpoint. A refresh token used twice signs the device out, because only a thief would do that.
- **Relays.** [iroh](https://www.iroh.computer/) relays carry encrypted QUIC traffic between players and cannot read it. `anyplay-relay` admits an endpoint only with a ticket the cloud signed for it.
- **One program, two shapes.** Self-hosters run one process with a SQLite file. AnyPlay's own cloud will run the same program on many nodes with PostgreSQL and NATS, and the same tests run on both ([ADR 0002](docs/adr/0002-one-program-two-shapes.md)).

## Repository

| Path | What it is |
|---|---|
| `crates/cloud` | `anyplay-cloud`, the server |
| `crates/relay` | `anyplay-relay`, an iroh relay that admits only endpoints with a ticket |
| `crates/ticket` | `anyplay-ticket`, the token format the cloud and the relay share |
| `crates/e2e` | a whole network in one process: the cloud, a relay that checks tickets, two net services and two fake games that exchange packets through a tunnel |
| `crates/load` | `anyplay-load`, a load harness that plays thousands of players against the API and event WebSockets; [numbers and method](docs/LOAD.md) |
| `deploy/` | Dockerfile, `compose.yaml`, Caddyfile and an example `.env` |
| `deploy/helm/` | the [Helm chart](docs/kubernetes.md) that runs the cloud as a cluster on Kubernetes, with an example CloudNativePG database and the files for the kind test |
| `fuzz/` | [fuzz targets](fuzz/README.md) for the token check, run every night |
| `docs/` | [architecture](docs/ARCHITECTURE.md), [settings](docs/configuration.md), [decisions](docs/adr) and [EU compliance drafts](docs/compliance/README.md) |

## Host your own

You need a machine with Docker, two host names that point at it (one for the cloud, one for the relay), an SMTP account that can send mail, and these ports open: 80, 443 and 8443 over TCP, and 7842 over UDP.

The build needs the network repository next to this one, because the cloud uses its API crate:

```text
workspace/
  cloud/       this repository
  network/     anyplay-gg/network
```

Then:

```sh
cd cloud/deploy
cp .env.example .env     # set the two host names, your e-mail address and the SMTP details
docker compose up -d
```

Caddy gets certificates and puts TLS in front of the cloud. The cloud creates its signing key in its data volume on the first start and writes the public half next to it; the relay waits for that file and for Caddy's certificate, then starts. Point a launcher at `https://<your cloud host name>`.

For a first try on your own machine, set `MAIL=log` in `.env`: sign-in codes then appear in `docker compose logs cloud`.

Back up the `cloud-data` volume. It holds the database and the signing key. Losing the key signs everybody out and makes every relay refuse its players until it is given the new public key.

Moderation, with `ADMIN_TOKEN` set in `.env`:

```sh
docker compose exec cloud anyplay-cloud admin reports
docker compose exec cloud anyplay-cloud admin ban <handle> --days 7 --reason "cheating"
docker compose exec cloud anyplay-cloud admin unban <handle>
```

Every setting is described in [docs/configuration.md](docs/configuration.md). That page also says how to run the relay on a machine of its own with a Let's Encrypt certificate.

## Development

```sh
cargo run -p anyplay-cloud       # http://127.0.0.1:8080, sign-in codes in the log, no TLS
cargo test --workspace
```

Without settings the cloud runs in development mode: codes go to the log, nothing is encrypted and anyone can sign up. `anyplay-cloud keygen` prints the public key a relay needs. The cloud finds the API crate at `../network/crates/api`, so clone both repositories side by side.

Every pull request runs formatting, Clippy with warnings as errors and all tests on Windows and Linux, builds the three images and starts them with the compose file. Both fuzz targets of the token check run for 15 minutes each every night.

## What the cloud never does

It never sees game files, never contacts Sony's servers and never stands in for a publisher's servers. The [clean line](https://github.com/anyplay-gg/network/blob/main/docs/adr/0003-the-clean-line.md) explains why. A game whose online part needs its publisher's servers stays offline.

## License

AGPL-3.0. If you run a modified cloud for other people, you must publish your changes.
