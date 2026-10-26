# Updating a running cloud

`deploy/update.sh` moves a deployment made with [`deploy/compose.yaml`](../deploy/compose.yaml) to a new version, checks that it came up and rolls back if it did not. There are two paths. The fast one, [`--image`](#deploying-a-prebuilt-image), pulls the images that CI built for a commit from GHCR. The fallback builds them on the server from given commits of this repository and of the network repository. Run it on the machine that hosts the cloud, from a checkout laid out as in the README (`cloud/` and `network/` side by side, `cloud/deploy/.env` filled in):

```sh
cloud/deploy/update.sh --cloud <commit> --image sha256:<cloud digest> --relay-image sha256:<relay digest> [--web-image sha256:<web digest>]   # fast path: pull the images CI built
cloud/deploy/update.sh                                   # fallback: build origin/main of the cloud with the network commit in its network.ref
cloud/deploy/update.sh --cloud 1a2b3c4 --network v0.4.0  # fallback with commits, tags or branches
cloud/deploy/update.sh --dry-run                         # show what would change, change nothing
```

| Option | Meaning |
|---|---|
| `--cloud REF`, `--network REF` | What to deploy. A branch name means its state on `origin`. `--cloud` defaults to `origin/main`. `--network` defaults to the commit in [`network.ref`](../network.ref) of the cloud commit being deployed (not of the checkout that is there now); `--network REF` overrides it. |
| `--no-backup` | Skip the backup that otherwise runs before every update (see [Backups](#backups)). Prints a warning; use it only when the backup itself is what is broken. |
| `--pre-hook CMD` | A command to run before the first change, for example a check of your own. If it fails the update stops and nothing has changed. Also `ANYPLAY_UPDATE_HOOK`. |
| `--timeout SECONDS` | How long the new version gets to become healthy and to answer. Default 180. Also `ANYPLAY_UPDATE_TIMEOUT`. |
| `--no-rollback` | Leave a failed version running, for looking at it. |
| `--dry-run` | Fetch and resolve the refs, print them, stop. |
| `--image DIGEST`, `--relay-image DIGEST` | Deploy the prebuilt images of these digests (`sha256:...`) instead of building them; needs `--cloud COMMIT`. See [Deploying a prebuilt image](#deploying-a-prebuilt-image). |
| `--web-image DIGEST` | Pull the website's prebuilt image (`ghcr.io/anyplaynet/web`, built by the web repository's CI) instead of building it; needs the website enabled (`compose.web.yaml`, see [The website](#the-website)). Can be combined with `--image` or used alone. |
| `--no-attestation` | With `--image` or `--web-image`: do not check the images against their attestation. Prints a warning. |

`ANYPLAY_UPDATE_URL` replaces the address that is checked (default `https://<CLOUD_DOMAIN>/v1/info`).

## What it does

1. Fetches both repositories and resolves the refs. Nothing has changed yet. It refuses to run when a checkout has uncommitted changes to tracked files, when another update runs, or when less than 5 GB of disk is free.
2. Checks what the backup needs, while nothing has changed: the data volume exists and the backup folder is writable. A missing volume counts as a first install (nothing to back up, skipped with a message) only when there is no cloud container either (`docker compose ps -a -q cloud` is empty). If a cloud container exists but the volume does not, its name differs from `anyplay_cloud-data` (it follows the compose project name, `COMPOSE_PROJECT_NAME` or `-p`) and the update stops and asks for `ANYPLAY_CLOUD_VOLUME`. Then it runs the pre-hook if there is one.
3. Tags the images that run now as `anyplay-rollback/<image>:previous`, so a rollback does not need the build cache.
4. Checks out the new commits (detached) and **builds while the old version keeps serving**. If the build fails, it puts the old commits back and stops; the running version was never touched. When the build has succeeded, it backs up the data volume with [`deploy/backup.sh`](../deploy/backup.sh), **right before the new version starts**: the build can take minutes during which the old version keeps writing, and a backup from before it would lose those writes. If the backup fails, or the newest archive is older than `ANYPLAY_BACKUP_MAX_AGE` (default 900 seconds), the old commits go back and the update stops with the running version untouched.
5. Runs `docker compose up -d --wait`. Compose replaces only the services whose image or settings changed. When the Caddyfile or a site file (`deploy/*.caddy`) changed, it also starts a new Caddy container: Caddy reads them through single-file mounts, which keep showing the old file after a checkout.
6. Checks that `/v1/info` answers over the public address, through Caddy and TLS, and that every container is running after a few seconds.
7. On success it prunes dangling images and build cache older than seven days. On failure it checks out the previous commits, puts the previous images back, starts them and checks again.

## Deploying a prebuilt image

Every merge to `main` builds the cloud and relay images in CI ([`image` workflow](../.github/workflows/image.yml)) and pushes them to `ghcr.io/anyplaynet/cloud` and `ghcr.io/anyplaynet/relay`, tagged with the commit and as `main`. Each image carries an attestation made by that workflow. The run's job summary prints the command:

```sh
cloud/deploy/update.sh --cloud <commit> --image sha256:<cloud digest> --relay-image sha256:<relay digest>
```

`--cloud` is the commit the images were built from (the compose files, the Caddyfile and `network.ref` come from that checkout). With `--image` step 4 above changes: instead of building the cloud and relay it pulls both images by digest, and it stops, with the running version untouched, when

- the pull fails (the server must be able to read ghcr.io, see below),
- an image's `org.opencontainers.image.revision` label is not the cloud commit being deployed, or
- `gh attestation verify oci://<image> --repo anyplaynet/cloud` rejects it. This check runs when `gh` is installed and logged in on the server; without `gh` the script warns and goes on, and `--no-attestation` skips it on purpose.

Caddy is still built on the server (a few cached layers), and so is the website unless you pass `--web-image` (below). Everything else is as before: the backup right before the new version starts, the rollback tags, the health check and the rollback. The pulled images are tagged `anyplay-cloud` and `anyplay-relay`, the names `compose.yaml` runs, so a later `docker compose up -d` by hand finds them.

**The website image.** The web repository builds its image the same way: every merge to its `main` pushes `ghcr.io/anyplaynet/web:<commit>` and `:main` with an attestation (its `docker` workflow; the job summary prints the digest). `--web-image sha256:<digest>` pulls it by digest, checks `gh attestation verify oci://<image> --repo anyplaynet/web` (same rules as above: with `gh`, skipped by `--no-attestation`, a warning without `gh`), tags it `anyplay-web` and leaves the website out of the build on the server, which saves 5 to 8 minutes. The website has its own commit, so the `org.opencontainers.image.revision` label is not compared with `--cloud`; the attestation is what ties the image to the web repository. The script stops with the running version untouched when the compose files have no `web` service. Without `--web-image` the website is built from the `web/` checkout as before.

To find the digests of the latest build: the job summary of the newest `image` run on `main`, or `docker buildx imagetools inspect ghcr.io/anyplaynet/cloud:main` (likewise `.../web:main`). To check an image by hand: `gh attestation verify oci://ghcr.io/anyplaynet/cloud@sha256:... --repo anyplaynet/cloud`.

**One-time setup on the server:** the packages must be readable. If `docker pull ghcr.io/anyplaynet/cloud:main` is denied, either make the packages (cloud, relay and, for `--web-image`, web) public (GitHub: the package's settings, "Change visibility") or run `docker login ghcr.io` with a token that has only `read:packages`. Without `--image` nothing changes: the build path is the fallback, for example when the `image` workflow is red. See [ADR 0007](adr/0007-images-are-built-in-ci-and-pulled.md).

## Which network revision

The cloud builds against one network commit everywhere: the single full SHA in [`network.ref`](../network.ref). CI, the Kubernetes job, fuzzing, the advisory check and `update.sh` read it, and the Docker image is built from the `network/` checkout that `update.sh` puts on that commit. Merging to the network's `main` therefore never changes what the cloud builds or deploys.

To move to newer network code, change `network.ref` to the network commit you need **in the same pull request** as the cloud change that needs it, so CI proves the pair. The workflow `network-drift` (nightly and on demand) builds the cloud's `main` against the network's `main` and goes red when that no longer works; it reports, it blocks nothing. A hotfix on the server can still pass `--network <ref>`.

## Downtime

The cloud keeps its state in one SQLite file, so there is exactly one cloud process: it stops and the new one starts, which takes a few seconds (the image is already built). The relay waits for the cloud and restarts with it; Caddy only restarts when its own image changed. Clients reconnect on their own. This is near-zero downtime, not zero. Zero downtime needs a second node, which means the Postgres setup of [ADR 0003](adr/0003-postgres-for-a-cluster.md) and the [Kubernetes chart](kubernetes.md).

## What a rollback does not undo

A new version may migrate the database on its first start. The old version does not undo that. If a rollback happens after a migration, or if the new version damaged data, restore the backup that `update.sh` took (below) instead of only starting the old code. A failed update prints the command: `restore with deploy/restore.sh <archive> --really`. The archive was taken right before the new version started, so it holds everything up to that moment; what the new version wrote after it (accounts, sign-ups, bans) is gone after a restore. The same is true of the old version's data if you roll back late: the longer the new version ran, the more is lost.

## Screenshots and clips

Players can send a screenshot (up to 10 MB) or a clip (up to 100 MB and 60 seconds) to a friend. The cloud keeps the file for 30 days in `media_dir` (default `cloud-data/media`, so the same volume as the database) and then deletes it.

- **Size the disk for it.** The worst case is `media_account_bytes` (1 GiB) per account for 30 days; in practice a small fraction. Watch the volume's free space.
- **A cluster needs a shared folder.** Every node must mount the same `media_dir`; a file uploaded through one node is read through another.
- **Back it up with the database** (the volume backup below takes it along). A restored database without the matching media folder shows media messages that cannot be opened until they expire; restore both from the same archive.
- Uploads and downloads of media are outside the 30 second request timeout of other routes. They have their own limit (10 minutes for an upload, four uploads at a time per node), so a reverse proxy in front must allow a request body of 100 MB and a request time of 10 minutes on `/v1/conversations/*/media`.

## Backups

The `cloud-data` volume holds everything the cloud cannot rebuild: the SQLite database (`anyplay-cloud.db`), `signing.key` (lose it and every token and relay ticket becomes invalid) and `ban.key` (lose it and banned e-mail hashes silently stop matching). [`deploy/backup.sh`](../deploy/backup.sh) copies the whole volume:

1. It checks that the backup folder is writable before touching anything.
2. It stops the `cloud` container, so SQLite checkpoints its write-ahead log and the copy is consistent. The relay and Caddy keep running; clients reconnect when the cloud is back.
3. A throwaway container (`python:3.13-alpine`, pinned by digest, `ANYPLAY_BACKUP_IMAGE`; the script pulls it before it stops the cloud, so a registry failure never leaves the cloud down; the cloud image needs no extra tool and there is no `sqlite3` CLI to install) packs the volume into `anyplay-cloud-<UTC time>.tar.gz` under a temporary name, `sync`s it, and the script renames it into place.
4. It starts the cloud again and waits for its health check, then logs how long the cloud was down (from the stop until healthy). This also happens when the copy failed (an exit trap), and the script then still exits non-zero.
5. It writes `<archive>.sha256` (standard format, `sha256sum -c` verifies it), sets both files to mode 0600 and deletes all but the newest 14 archives.

```sh
cloud/deploy/backup.sh                       # into deploy/backups, keep 14
cloud/deploy/backup.sh --dir /srv/backups --keep 30
```

`update.sh` runs it after the build, right before the new version starts (the window between the archive and the start is the stop and start of the cloud, not the build), and logs `backup ok: <archive>, <n>s old`. The settings `ANYPLAY_BACKUP_DIR` (default `deploy/backups`), `ANYPLAY_BACKUP_KEEP`, `ANYPLAY_BACKUP_MAX_AGE` and `ANYPLAY_CLOUD_VOLUME` can be exported or set in `deploy/.env`. Keep the folder on another disk, or copy it off the machine: a backup next to the data it protects does not survive a lost disk. The archives hold the signing key and personal data; they are mode 0600, and anything you copy them to needs the same care (and a retention period, see gap G10 in the [data inventory](compliance/data-inventory.md): a restore brings back accounts that were deleted after the backup, so replay deletions after a restore).

For a backup between updates, install the [timer and service example](../deploy/systemd/) (nightly, skips a run while an update holds the lock). A nightly timer bounds the loss between updates to a day; the one in front of every update bounds it to the update.

`--no-backup` on `update.sh` skips the step and prints a warning. Without the flag, an update stops when no fresh archive could be made.

### Restore drill

Prove an archive works before you need it, without touching the running cloud:

```sh
cloud/deploy/restore.sh deploy/backups/anyplay-cloud-20261025T031500Z.tar.gz
```

It verifies the manifest, empties and fills a scratch volume (`anyplay_restore-test`, or `--volume NAME`), and in a throwaway container as the cloud's user runs `PRAGMA integrity_check` and `SELECT count(*) FROM accounts`. It prints both and the wall-clock time, and exits non-zero on any failure. Compare the account count with what you expect. `docker volume rm anyplay_restore-test` removes the scratch copy.

To restore for real, add `--really`. It first unpacks and checks the archive in the scratch volume, exactly as the drill does, and touches nothing real if that fails. Only then it stops the whole stack, saves the current volume next to the archive as `anyplay-cloud-pre-restore-<UTC time>.tar.gz` (mode 0600, with a manifest; `backup.sh`'s retention never deletes it, you do), replaces the contents of the real volume (database, `signing.key`, `ban.key`) with the archive, checks the database again and starts the stack. If a step fails after the stop, the old volume is unpacked from the safety copy and the old stack is started again. Run the drill first, and note the commits you are on; an archive made by a newer version may hold a migration the older code does not know.

Test the scripts themselves with `deploy/test-backup.sh` (needs Docker; uses throwaway volumes only).

### Measured

On a development PC (Docker Desktop, WSL 2 backend), a data volume with a 122 MB database of 250,000 account rows:

| Step | Time |
|---|---|
| `backup.sh` copy (cloud stopped during it) | 4.0 s wall-clock (archive 66 MB) |
| `restore.sh` drill into the scratch volume, with integrity check and row count | 3.3 s |

The downtime of the cloud during a backup was **not measured with the real cloud image**; the copy figure above is only the time of the copy itself. The cloud is down for the SIGTERM stop, the copy, the start and the wait for the health check; the image declares `HEALTHCHECK --interval=15s --start-period=20s`, so "healthy again" comes at least 15 s after the start. Plan for tens of seconds, not a few. `backup.sh` logs `the cloud was down for about Ns, from the stop until its health check passed again` on every run: read the real figure from the first backup on the production machine. Record the restore time there too (it depends on its disk): recovery time is the drill time plus starting the stack.

## Rolling back by hand

The previous commits are in the log of the update (`cloud abc -> def`). Check them out in both repositories, tag the old images back (`docker tag anyplay-rollback/anyplay-cloud:previous anyplay-cloud`, and the same for `anyplay-relay` and `anyplay-caddy`) and run `docker compose up -d --no-build` in `cloud/deploy`. Or run `update.sh --cloud <old> --network <old>`, which rebuilds them.

## Logs and health checks

`compose.yaml` keeps three files of 10 MB per container (`json-file` driver), so container logs cannot fill the disk. The cloud, the relay and Caddy each have a health check; `docker compose ps` shows it, and the update waits for it.

## The website

The landing page (anyplaynet/web) can run on the cloud's host name: Caddy sends `/` and the files the page loads to it and everything else, the account pages under `/web` included, to the cloud. It is optional and off by default.

1. Check out [anyplaynet/web](https://github.com/anyplaynet/web) beside `cloud/` and `network/` (or set `WEB_DIR` in `.env` to its path).
2. Add `COMPOSE_FILE=compose.yaml:compose.web.yaml` to `deploy/.env`. Compose reads it there, so `update.sh` and every `docker compose` call include the website from then on.
3. Run `update.sh` as usual. It builds the website from the checkout as it is; update that checkout with `git pull` first. Or skip the checkout and the build: pass the digest of the website image CI built, `update.sh ... --web-image sha256:<digest>` (see [Deploying a prebuilt image](#deploying-a-prebuilt-image)). `compose.web.yaml` also reads `WEB_IMAGE` (an image reference) from `.env`, for running a given image by hand with `docker compose pull web && docker compose up -d`.

4. Set `ANYPLAY_CLOUD_PORTAL_EXTERNAL=true` (with `ANYPLAY_CLOUD_PUBLIC_URL`) in the cloud's environment, so the launcher's sign-in opens the website's `/sign-in` and `/create` pages instead of the cloud's own.

The website also serves the legal notice, the privacy notice and the terms of use at `/legal`, `/privacy` and `/terms` (`deploy/web.caddy` routes them; a new path must be added to its `@website` matcher, or the cloud answers it with a 404). Point the cloud at them in `deploy/.env`: `PRIVACY_URL=https://<your host>/privacy`, `TERMS_URL=https://<your host>/terms` and `TERMS_VERSION` equal to the version the terms page shows (without `TERMS_VERSION` the sign-up asks for no acceptance). The website takes its instance details from the environment of the `web` service, which `compose.web.yaml` fills from `deploy/.env`: `SITE_URL`, `LEGAL_OPERATOR_NAME`, `LEGAL_OPERATOR_STREET`, `LEGAL_OPERATOR_CITY`, `LEGAL_OPERATOR_COUNTRY`, `LEGAL_CONTACT_EMAIL`, `LEGAL_PRIVACY_EMAIL`, `LEGAL_ABUSE_EMAIL`, optionally `LEGAL_AUTHORITY_NAME`, `_STREET`, `_CITY`, `_SITE` and `COMMUNITY_DISCORD_URL`. They are read at request time, so a restart of `web` is enough. Nothing has a default: without the operator details the legal notice says the instance has not published them, and the web container logs one warning at start-up that lists what is missing. The variables are described in the [web README](https://github.com/anyplaynet/web#run-your-own-anyplay-website). The legal texts are written for the AnyPlay service; have them reviewed for your own situation.

Notifications need the website to serve `/sw.js` (the service worker), `/manifest.webmanifest` and `/icons/*` from the root of the host name; `deploy/web.caddy` sends them there. The worker must not be cached for long: the web app sends it with `Cache-Control: no-cache`, so a new version reaches browsers within a day at the latest. Set the VAPID settings of [Notifications](configuration.md#notifications-web-push) in the cloud's environment.

To take the website down again, remove the `COMPOSE_FILE` line and run `docker compose up -d --remove-orphans`.

## Linked accounts

The account web lets players link Discord and Steam ([configuration](configuration.md)). A service stays off until its setting is in `deploy/.env`:

- `DISCORD_CLIENT_ID` and `DISCORD_CLIENT_SECRET`: the OAuth2 client of the cloud's Discord application, with `https://<CLOUD_DOMAIN>/web/links/discord/callback` as a redirect in the Discord Developer Portal.
- `STEAM_API_KEY`: a Steam Web API key, for the player's Steam name and picture.

- `VAPID_PRIVATE_KEY` and `VAPID_SUBJECT`: the key of Web Push notifications and a contact (`mailto:...`). Make the key with `docker compose run --rm cloud vapid-keys`; see [Notifications](configuration.md#notifications-web-push). Keep the key: a new one drops every subscription.

Run `deploy/update.sh` after a change so the cloud container picks them up.
