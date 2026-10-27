# AnyPlay Cloud

The central services of the [AnyPlay network](https://github.com/anyplaynet/network). Players sign in, find friends, party up and play together through them. The cloud runs live at [anyplaynet.com](https://anyplaynet.com), and you can run your own from this repository.

## What it does

- **Accounts:** passwordless sign-in by e-mail code, a refresh token per device, passkeys and account settings on the web, export and deletion of all data.
- **Friends and presence:** requests, blocks, who is online and what they play.
- **Sessions, parties and voice signalling:** join and invite, capacity and version checks, and the signalling that sets up party voice between players. Voice itself runs between players, not through the cloud.
- **Messages:** one-to-one and group conversations, message requests, reactions, edit and unsend.
- **Profiles, feed and trophies:** profile pages with per-field visibility, a feed of friends' activity, trophy records with rarity and gamer cards.
- **Find Players (LFG):** listings to look for people to play with.
- **Share play:** a host streams the game they run to one party member; the cloud holds the invitation and the record, never the media. Off unless `share_enabled` is set.
- **Media:** screenshots and clips sent to friends, kept for 30 days.
- **Moderation:** reports, bans, rate limits and an admin command line.
- **Relay with ticket check:** `anyplay-relay` carries encrypted game traffic between players who cannot connect directly, and admits only endpoints holding a ticket the cloud signed.
- **Web portal API:** the JSON API behind the [account web](https://github.com/anyplaynet/web).

[docs/features.md](docs/features.md) has the details of each service. The wire format is the [cloud API](https://github.com/anyplaynet/network/blob/main/spec/cloud-api.md) in the network repository.

## Status

The cloud at anyplaynet.com runs as a single node: one process and one SQLite file. The multi-node setup with PostgreSQL and NATS is written and tested in CI, including a two-node cluster test and a Helm chart, but it has not carried real players yet. Regions are not built. The API may still change while the version is 0. Matchmaking, leaderboards and save backups are planned. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#stages).

## Run your own

You need a machine with Docker, two host names that point at it, an SMTP account and the ports 80, 443 and 8443 (TCP) and 7842 (UDP). Check out this repository and [anyplaynet/network](https://github.com/anyplaynet/network) side by side, then:

```sh
cd cloud/deploy
cp .env.example .env     # set the host names, your e-mail address and the SMTP details
docker compose up -d
```

Point a launcher at `https://<your cloud host name>`. Ports, settings and backups are in [docs/self-hosting.md](docs/self-hosting.md) and [docs/configuration.md](docs/configuration.md).

To update, run [`deploy/update.sh`](deploy/update.sh). CI builds the cloud and relay images to `ghcr.io/anyplaynet/cloud` and `ghcr.io/anyplaynet/relay` on every merge to `main`, and `update.sh --image` deploys one by digest, after a backup, with a health check and a rollback. Without `--image` it builds on the server. Both paths are in [docs/deploy.md](docs/deploy.md). For Kubernetes, see [docs/kubernetes.md](docs/kubernetes.md).

## What's in the repository

| Path | What it is |
|---|---|
| `crates/cloud` | `anyplay-cloud`, the server |
| `crates/relay` | `anyplay-relay`, an iroh relay that admits only endpoints with a ticket |
| `crates/ticket` | `anyplay-ticket`, the token format the cloud and the relay share |
| `crates/e2e` | a whole network in one process: the cloud, a relay, two net services and two fake games |
| `crates/load` | `anyplay-load`, a load harness ([numbers and method](docs/LOAD.md)) |
| `deploy/` | Dockerfile, `compose.yaml`, Caddyfile, update and backup scripts, the Helm chart |
| `docs/` | [features](docs/features.md), [architecture](docs/ARCHITECTURE.md), [settings](docs/configuration.md), [deploying](docs/deploy.md), [moderation](docs/moderation.md), [decisions](docs/adr), [EU compliance drafts](docs/compliance/README.md), [supply chain](docs/supply-chain.md) |

## Build and contribute

```sh
cargo run -p anyplay-cloud       # http://127.0.0.1:8080, sign-in codes in the log
cargo test --workspace
```

The cloud needs [anyplaynet/network](https://github.com/anyplaynet/network) checked out next to it at the commit in `network.ref`. Every pull request runs formatting, Clippy and the tests on Windows and Linux. Until the network is live, only the AnyPlay team opens issues and pull requests here; the shared rules are in [CONTRIBUTING.md](https://github.com/anyplaynet/.github/blob/main/CONTRIBUTING.md).

## Community

Questions, ideas and bug reports: the [AnyPlay Discord](https://discord.gg/9zYqf8fEsh). More about AnyPlay at [anyplaynet.com](https://anyplaynet.com).

## License

AGPL-3.0. If you run a modified cloud for other people, you must publish your changes.

AnyPlay is a community project, not affiliated with the AnyPS5 maintainers or any platform holder. The cloud never sees game files and never stands in for a publisher service that still runs ([what it never does](docs/features.md#what-the-cloud-never-does)).
