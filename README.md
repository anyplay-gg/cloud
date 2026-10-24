# AnyPlay Cloud

The central services of the AnyPlay network: accounts, friends, presence, sessions, invitations and relay tickets, plus a relay that only admits players who hold one. The design and the words we use are in [anyplay-gg/network](https://github.com/anyplay-gg/network); the API this cloud speaks is its [cloud API](https://github.com/anyplay-gg/network/blob/main/spec/cloud-api.md).

**Status: version 0.** Everything in the cloud API works and is tested. Matchmaking queues, leaderboards and save backups are not built yet. The API is still a draft and changes without notice until the first public cloud runs.

## What is in this repository

| Path | What it is |
|---|---|
| `crates/cloud` | `anyplay-cloud`, the server. Durable data is in one SQLite file ([why](docs/adr/0001-sqlite.md)); presence, sessions and invitations live in memory. |
| `crates/relay` | `anyplay-relay`, an [iroh](https://www.iroh.computer/) relay that admits an endpoint only with a ticket the cloud signed for it. |
| `crates/ticket` | `anyplay-ticket`, the signed token format that the cloud and the relay share. |
| `deploy/` | Dockerfile, `compose.yaml`, Caddyfile and an example `.env`. |
| `fuzz/` | [Fuzz targets](fuzz/README.md) for the token check, run every night. |

## Hosting your own

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

Without settings the cloud runs in development mode: codes go to the log, nothing is encrypted and anyone can sign up. `anyplay-cloud keygen` prints the public key a relay needs.

The cloud finds the API crate at `../network/crates/api`, so clone both repositories side by side.

`crates/e2e` runs a whole network in one process: the cloud, a relay that checks tickets, two net services driven as a launcher drives them, and two fake games that exchange packets through the tunnel.

## What the cloud never does

It never sees game files, never contacts Sony's servers and never stands in for a publisher's servers. The [clean line](https://github.com/anyplay-gg/network/blob/main/docs/adr/0003-the-clean-line.md) explains why.

## License

AGPL-3.0. If you run a modified cloud for other people, you must publish your changes.
