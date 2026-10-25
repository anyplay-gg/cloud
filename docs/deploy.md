# Updating a running cloud

`deploy/update.sh` moves a deployment made with [`deploy/compose.yaml`](../deploy/compose.yaml) to given commits of this repository and of the network repository, checks that it came up and rolls back if it did not. Run it on the machine that hosts the cloud, from a checkout laid out as in the README (`cloud/` and `network/` side by side, `cloud/deploy/.env` filled in):

```sh
cloud/deploy/update.sh                                   # origin/main of the cloud, the network commit in its network.ref
cloud/deploy/update.sh --cloud 1a2b3c4 --network v0.4.0  # commits, tags or branches
cloud/deploy/update.sh --dry-run                         # show what would change, change nothing
```

| Option | Meaning |
|---|---|
| `--cloud REF`, `--network REF` | What to deploy. A branch name means its state on `origin`. `--cloud` defaults to `origin/main`. `--network` defaults to the commit in [`network.ref`](../network.ref) of the cloud commit being deployed (not of the checkout that is there now); `--network REF` overrides it. |
| `--no-backup` | Skip the backup that otherwise runs before every update (see [Backups](#backups)). Prints a warning; use it only when the backup itself is what is broken. |
| `--pre-hook CMD` | A command to run before the first change, after the backup, for example a check of your own. If it fails the update stops and nothing has changed. Also `ANYPLAY_UPDATE_HOOK`. |
| `--timeout SECONDS` | How long the new version gets to become healthy and to answer. Default 180. Also `ANYPLAY_UPDATE_TIMEOUT`. |
| `--no-rollback` | Leave a failed version running, for looking at it. |
| `--dry-run` | Fetch and resolve the refs, print them, stop. |

`ANYPLAY_UPDATE_URL` replaces the address that is checked (default `https://<CLOUD_DOMAIN>/v1/info`).

## What it does

1. Fetches both repositories and resolves the refs. Nothing has changed yet. It refuses to run when a checkout has uncommitted changes to tracked files, when another update runs, or when less than 5 GB of disk is free.
2. Backs up the data volume with [`deploy/backup.sh`](../deploy/backup.sh), then runs the pre-hook if there is one. If the backup fails, or the newest archive is older than `ANYPLAY_BACKUP_MAX_AGE` (default 900 seconds), the update stops and nothing has changed. Without a `cloud-data` volume (a first install) there is nothing to back up and the step is skipped with a message.
3. Tags the images that run now as `anyplay-rollback/<image>:previous`, so a rollback does not need the build cache.
4. Checks out the new commits (detached) and **builds while the old version keeps serving**. If the build fails, it puts the old commits back and stops; the running version was never touched.
5. Runs `docker compose up -d --wait`. Compose replaces only the services whose image or settings changed. When the Caddyfile or a site file (`deploy/*.caddy`) changed, it also starts a new Caddy container: Caddy reads them through single-file mounts, which keep showing the old file after a checkout.
6. Checks that `/v1/info` answers over the public address, through Caddy and TLS, and that every container is running after a few seconds.
7. On success it prunes dangling images and build cache older than seven days. On failure it checks out the previous commits, puts the previous images back, starts them and checks again.

## Which network revision

The cloud builds against one network commit everywhere: the single full SHA in [`network.ref`](../network.ref). CI, the Kubernetes job, fuzzing, the advisory check and `update.sh` read it, and the Docker image is built from the `network/` checkout that `update.sh` puts on that commit. Merging to the network's `main` therefore never changes what the cloud builds or deploys.

To move to newer network code, change `network.ref` to the network commit you need **in the same pull request** as the cloud change that needs it, so CI proves the pair. The workflow `network-drift` (nightly and on demand) builds the cloud's `main` against the network's `main` and goes red when that no longer works; it reports, it blocks nothing. A hotfix on the server can still pass `--network <ref>`.

## Downtime

The cloud keeps its state in one SQLite file, so there is exactly one cloud process: it stops and the new one starts, which takes a few seconds (the image is already built). The relay waits for the cloud and restarts with it; Caddy only restarts when its own image changed. Clients reconnect on their own. This is near-zero downtime, not zero. Zero downtime needs a second node, which means the Postgres setup of [ADR 0003](adr/0003-postgres-for-a-cluster.md) and the [Kubernetes chart](kubernetes.md).

## What a rollback does not undo

A new version may migrate the database on its first start. The old version does not undo that. If a rollback happens after a migration, or if the new version damaged data, restore the backup that `update.sh` took just before (below) instead of only starting the old code. A failed update prints the command: `restore with deploy/restore.sh <archive> --really`.

## Backups

The `cloud-data` volume holds everything the cloud cannot rebuild: the SQLite database (`anyplay-cloud.db`), `signing.key` (lose it and every token and relay ticket becomes invalid) and `ban.key` (lose it and banned e-mail hashes silently stop matching). [`deploy/backup.sh`](../deploy/backup.sh) copies the whole volume:

1. It checks that the backup folder is writable before touching anything.
2. It stops the `cloud` container, so SQLite checkpoints its write-ahead log and the copy is consistent. The relay and Caddy keep running; clients reconnect when the cloud is back.
3. A throwaway container (`python:3.13-alpine`, `ANYPLAY_BACKUP_IMAGE`; the cloud image needs no extra tool and there is no `sqlite3` CLI to install) packs the volume into `anyplay-cloud-<UTC time>.tar.gz` under a temporary name, `sync`s it, and the script renames it into place.
4. It starts the cloud again and waits for its health check. This also happens when the copy failed (an exit trap), and the script then still exits non-zero.
5. It writes `<archive>.sha256` (standard format, `sha256sum -c` verifies it), sets both files to mode 0600 and deletes all but the newest 14 archives.

```sh
cloud/deploy/backup.sh                       # into deploy/backups, keep 14
cloud/deploy/backup.sh --dir /srv/backups --keep 30
```

`update.sh` runs it before every update, so the newest archive is seconds old when the first change happens, and logs `backup ok: <archive>, <n>s old`. The settings `ANYPLAY_BACKUP_DIR` (default `deploy/backups`), `ANYPLAY_BACKUP_KEEP`, `ANYPLAY_BACKUP_MAX_AGE` and `ANYPLAY_CLOUD_VOLUME` can be exported or set in `deploy/.env`. Keep the folder on another disk, or copy it off the machine: a backup next to the data it protects does not survive a lost disk. The archives hold the signing key and personal data; they are mode 0600, and anything you copy them to needs the same care (and a retention period, see gap G10 in the [data inventory](compliance/data-inventory.md): a restore brings back accounts that were deleted after the backup, so replay deletions after a restore).

For a backup between updates, install the [timer and service example](../deploy/systemd/) (nightly, skips a run while an update holds the lock). A nightly timer bounds the loss between updates to a day; the one in front of every update bounds it to the update.

`--no-backup` on `update.sh` skips the step and prints a warning. Without the flag, an update stops when no fresh archive could be made.

### Restore drill

Prove an archive works before you need it, without touching the running cloud:

```sh
cloud/deploy/restore.sh deploy/backups/anyplay-cloud-20261025T031500Z.tar.gz
```

It verifies the manifest, empties and fills a scratch volume (`anyplay_restore-test`, or `--volume NAME`), and in a throwaway container as the cloud's user runs `PRAGMA integrity_check` and `SELECT count(*) FROM accounts`. It prints both and the wall-clock time, and exits non-zero on any failure. Compare the account count with what you expect. `docker volume rm anyplay_restore-test` removes the scratch copy.

To restore for real, add `--really`: it stops the whole stack, replaces the contents of the real volume (database, `signing.key`, `ban.key`) with the archive, checks the database again and starts the stack. Run the drill first, and note the commits you are on; an archive made by a newer version may hold a migration the older code does not know.

Test the scripts themselves with `deploy/test-backup.sh` (needs Docker; uses throwaway volumes only).

### Measured

On a development PC (Docker Desktop, WSL 2 backend), a data volume with a 122 MB database of 250,000 account rows:

| Step | Time |
|---|---|
| `backup.sh` copy (cloud stopped during it) | 4.0 s wall-clock (archive 66 MB) |
| `restore.sh` drill into the scratch volume, with integrity check and row count | 3.3 s |

The cloud is stopped for the copy plus the container's shutdown and start, which the test measured at 5 s end to end with a stand-in container. Record the same numbers from the production machine after the first drill there (they depend on its disk): recovery time is the drill time plus starting the stack.

## Rolling back by hand

The previous commits are in the log of the update (`cloud abc -> def`). Check them out in both repositories, tag the old images back (`docker tag anyplay-rollback/anyplay-cloud:previous anyplay-cloud`, and the same for `anyplay-relay` and `anyplay-caddy`) and run `docker compose up -d --no-build` in `cloud/deploy`. Or run `update.sh --cloud <old> --network <old>`, which rebuilds them.

## Logs and health checks

`compose.yaml` keeps three files of 10 MB per container (`json-file` driver), so container logs cannot fill the disk. The cloud, the relay and Caddy each have a health check; `docker compose ps` shows it, and the update waits for it.

## The website

The landing page (anyplaynet/web) can run on the cloud's host name: Caddy sends `/` and the files the page loads to it and everything else, the account pages under `/web` included, to the cloud. It is optional and off by default.

1. Check out [anyplaynet/web](https://github.com/anyplaynet/web) beside `cloud/` and `network/` (or set `WEB_DIR` in `.env` to its path).
2. Add `COMPOSE_FILE=compose.yaml:compose.web.yaml` to `deploy/.env`. Compose reads it there, so `update.sh` and every `docker compose` call include the website from then on.
3. Run `update.sh` as usual. It builds the website from the checkout as it is; update that checkout with `git pull` first.

4. Set `ANYPLAY_CLOUD_PORTAL_EXTERNAL=true` (with `ANYPLAY_CLOUD_PUBLIC_URL`) in the cloud's environment, so the launcher's sign-in opens the website's `/sign-in` and `/create` pages instead of the cloud's own.

The website also serves the privacy notice and the terms of use at `/privacy` and `/terms` (`deploy/web.caddy` routes them). Point the cloud at them in `deploy/.env`: `PRIVACY_URL=https://<your host>/privacy`, `TERMS_URL=https://<your host>/terms` and `TERMS_VERSION` equal to the version the terms page shows. The texts name AnyPlay as the operator and its e-mail address; if you run your own cloud, publish your own texts instead.

Notifications need the website to serve `/sw.js` (the service worker), `/manifest.webmanifest` and `/icons/*` from the root of the host name; `deploy/web.caddy` sends them there. The worker must not be cached for long: the web app sends it with `Cache-Control: no-cache`, so a new version reaches browsers within a day at the latest. Set the VAPID settings of [Notifications](configuration.md#notifications-web-push) in the cloud's environment.

To take the website down again, remove the `COMPOSE_FILE` line and run `docker compose up -d --remove-orphans`.

## Linked accounts

The account web lets players link Discord and Steam ([configuration](configuration.md)). A service stays off until its setting is in `deploy/.env`:

- `DISCORD_CLIENT_ID` and `DISCORD_CLIENT_SECRET`: the OAuth2 client of the cloud's Discord application, with `https://<CLOUD_DOMAIN>/web/links/discord/callback` as a redirect in the Discord Developer Portal.
- `STEAM_API_KEY`: a Steam Web API key, for the player's Steam name and picture.

- `VAPID_PRIVATE_KEY` and `VAPID_SUBJECT`: the key of Web Push notifications and a contact (`mailto:...`). Make the key with `docker compose run --rm cloud vapid-keys`; see [Notifications](configuration.md#notifications-web-push). Keep the key: a new one drops every subscription.

Run `deploy/update.sh` after a change so the cloud container picks them up.
