# Updating a running cloud

`deploy/update.sh` moves a deployment made with [`deploy/compose.yaml`](../deploy/compose.yaml) to given commits of this repository and of the network repository, checks that it came up and rolls back if it did not. Run it on the machine that hosts the cloud, from a checkout laid out as in the README (`cloud/` and `network/` side by side, `cloud/deploy/.env` filled in):

```sh
cloud/deploy/update.sh                                   # origin/main of both
cloud/deploy/update.sh --cloud 1a2b3c4 --network v0.4.0  # commits, tags or branches
cloud/deploy/update.sh --dry-run                         # show what would change, change nothing
```

| Option | Meaning |
|---|---|
| `--cloud REF`, `--network REF` | What to deploy. A branch name means its state on `origin`. Default `origin/main`. |
| `--pre-hook CMD` | A command to run before the first change, for example your backup. If it fails the update stops and nothing has changed. Also `ANYPLAY_UPDATE_HOOK`. |
| `--timeout SECONDS` | How long the new version gets to become healthy and to answer. Default 180. Also `ANYPLAY_UPDATE_TIMEOUT`. |
| `--no-rollback` | Leave a failed version running, for looking at it. |
| `--dry-run` | Fetch and resolve the refs, print them, stop. |

`ANYPLAY_UPDATE_URL` replaces the address that is checked (default `https://<CLOUD_DOMAIN>/v1/info`).

## What it does

1. Fetches both repositories and resolves the refs. Nothing has changed yet. It refuses to run when a checkout has uncommitted changes to tracked files, when another update runs, or when less than 5 GB of disk is free.
2. Runs the pre-hook.
3. Tags the images that run now as `anyplay-rollback/<image>:previous`, so a rollback does not need the build cache.
4. Checks out the new commits (detached) and **builds while the old version keeps serving**. If the build fails, it puts the old commits back and stops; the running version was never touched.
5. Runs `docker compose up -d --wait`. Compose replaces only the services whose image or settings changed.
6. Checks that `/v1/info` answers over the public address, through Caddy and TLS, and that every container is running after a few seconds.
7. On success it prunes dangling images and build cache older than seven days. On failure it checks out the previous commits, puts the previous images back, starts them and checks again.

## Downtime

The cloud keeps its state in one SQLite file, so there is exactly one cloud process: it stops and the new one starts, which takes a few seconds (the image is already built). The relay waits for the cloud and restarts with it; Caddy only restarts when its own image changed. Clients reconnect on their own. This is near-zero downtime, not zero. Zero downtime needs a second node, which means the Postgres setup of [ADR 0003](adr/0003-postgres-for-a-cluster.md) and the [Kubernetes chart](kubernetes.md).

## What a rollback does not undo

A new version may migrate the database on its first start. The old version does not undo that. If a rollback happens after a migration, or if the new version damaged data, restore the backup taken by the pre-hook (the database, `signing.key` and `ban.key` live in the `cloud-data` volume) instead of only starting the old code. Take the backup with the pre-hook every time:

```sh
cloud/deploy/update.sh --pre-hook /usr/local/sbin/my-backup
```

## Rolling back by hand

The previous commits are in the log of the update (`cloud abc -> def`). Check them out in both repositories, tag the old images back (`docker tag anyplay-rollback/anyplay-cloud:previous anyplay-cloud`, and the same for `anyplay-relay` and `anyplay-caddy`) and run `docker compose up -d --no-build` in `cloud/deploy`. Or run `update.sh --cloud <old> --network <old>`, which rebuilds them.

## Logs and health checks

`compose.yaml` keeps three files of 10 MB per container (`json-file` driver), so container logs cannot fill the disk. The cloud, the relay and Caddy each have a health check; `docker compose ps` shows it, and the update waits for it.

## The website

The landing page (anyplay-gg/web) can run on the cloud's host name: Caddy sends `/` and the files the page loads to it and everything else, the account pages under `/web` included, to the cloud. It is optional and off by default.

1. Check out [anyplay-gg/web](https://github.com/anyplay-gg/web) beside `cloud/` and `network/` (or set `WEB_DIR` in `.env` to its path).
2. Add `COMPOSE_FILE=compose.yaml:compose.web.yaml` to `deploy/.env`. Compose reads it there, so `update.sh` and every `docker compose` call include the website from then on.
3. Run `update.sh` as usual. It builds the website from the checkout as it is; update that checkout with `git pull` first.

4. Set `ANYPLAY_CLOUD_PORTAL_EXTERNAL=true` (with `ANYPLAY_CLOUD_PUBLIC_URL`) in the cloud's environment, so the launcher's sign-in opens the website's `/sign-in` and `/create` pages instead of the cloud's own.

To take the website down again, remove the `COMPOSE_FILE` line and run `docker compose up -d --remove-orphans`.
