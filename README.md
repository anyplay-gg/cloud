# AnyPlay Cloud

The services behind the [AnyPlay network](https://github.com/anyplaynet/network): accounts, friends, presence, sessions, invitations, parties and messages, moderation, and relays that carry game traffic when two players cannot reach each other directly. One program runs a whole cloud on a home server with `docker compose up`. The same program is being built to run as a cluster.

## Use it

AnyPlay runs its own cloud at [anyplaynet.com](https://anyplaynet.com), with the [website and account web](https://github.com/anyplaynet/web). Players need nothing else: the [AnyPlay Launcher](https://github.com/anyplaynet/launcher) uses it by default.

To run your own, on a machine with Docker, two host names and an SMTP account:

```sh
cd deploy
cp .env.example .env     # set the host names, your e-mail address and the SMTP details
docker compose up -d
```

Then point a launcher at `https://<your cloud host name>`. The full steps, ports, backups and moderation are in [docs/self-hosting.md](docs/self-hosting.md).

## What's inside

| Path | What it is |
|---|---|
| `crates/cloud` | `anyplay-cloud`, the server |
| `crates/relay` | `anyplay-relay`, an iroh relay that admits only endpoints with a ticket |
| `crates/ticket` | `anyplay-ticket`, the token format the cloud and the relay share |
| `crates/e2e` | a whole network in one process: the cloud, a relay, two net services and two fake games |
| `crates/load` | `anyplay-load`, a load harness for the API and event WebSockets ([numbers and method](docs/LOAD.md)) |
| `deploy/` | Dockerfile, `compose.yaml`, Caddyfile, an example `.env` and the [Helm chart](docs/kubernetes.md) |
| `docs/` | [features](docs/features.md), [architecture](docs/ARCHITECTURE.md), [settings](docs/configuration.md), [decisions](docs/adr), [EU compliance drafts](docs/compliance/README.md), [supply chain](docs/supply-chain.md) |

## Status

Version 0, open to everyone. The services in [docs/features.md](docs/features.md) work and are tested end to end; the API may still change while the version is 0. Matchmaking, leaderboards and save backups are planned. Found a problem? Tell us on Discord.

## Build and contribute

```sh
cargo run -p anyplay-cloud       # http://127.0.0.1:8080, sign-in codes in the log
cargo test --workspace
```

The cloud needs [anyplaynet/network](https://github.com/anyplaynet/network) checked out next to it at the commit in `network.ref`. Every pull request runs formatting, Clippy and the tests on Windows and Linux. Until the network is live, only the AnyPlay team opens issues and pull requests; the shared rules are in [CONTRIBUTING.md](https://github.com/anyplaynet/.github/blob/main/CONTRIBUTING.md).

## Community

Questions and ideas: the [AnyPlay Discord](https://discord.gg/9zYqf8fEsh). More about AnyPlay at [anyplaynet.com](https://anyplaynet.com).

## License

AGPL-3.0. If you run a modified cloud for other people, you must publish your changes.

AnyPlay is a community project, not affiliated with the AnyPS5 maintainers or any platform holder. The cloud never sees game files and never stands in for a publisher service that still runs ([what it never does](docs/features.md#what-the-cloud-never-does)).
