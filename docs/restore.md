# Restore runbook

How to get the cloud back from a backup. Written for the operator at three in the morning: do the steps in order and read what each prints. The background (what a backup holds, retention, the deletion ledger) is in [deploy.md](deploy.md#backups).

## What is proven, and how

Every pull request and every Monday the workflow [`restore-drill`](../.github/workflows/restore-drill.yml) does this with a seeded cloud (three accounts, one friendship, one party):

1. takes a snapshot of the cloud **while it runs** (`deploy/snapshot.py`, SQLite's online backup API),
2. checks the archive against its SHA-256 manifest and restores it with `deploy/restore.sh` into a scratch volume (`PRAGMA integrity_check` and the row count),
3. unpacks the archive into a clean folder, starts a new cloud on it and checks that a token from before the backup still works (the signing key came back), that a player signs in with an e-mail code, that the friendship and the party are there, and that an account made after the snapshot is not.

The steps below are the same commands. What the drill cannot show is your machine's disk and size: the times in the table are from a development PC and CI; **take your own on the first real restore** and write them down here.

| Step | Measured | Where |
|---|---|---|
| Archive of a 122 MB database (250,000 accounts) | 4.0 s | development PC, `backup.sh` |
| `restore.sh` drill of that archive (scratch volume, integrity check, row count) | 3.3 s | development PC |
| Seed, snapshot, drill and verify of the CI database | see the job summary of the latest `restore-drill` run | GitHub runner |
| Production restore, start to the first successful sign-in | **not measured yet** | plan on a few minutes: the stop, the copy, the start and the health check (about 15 to 35 s) |

## Before you restore

1. Say why. A restore **loses** everything written since the archive: accounts, sign-ups, friend requests, messages. If the problem is only a bad release, `update.sh` rolls back the code ([deploy.md](deploy.md#rolling-back-by-hand)); restore only when data is damaged or a migration went wrong.
2. Find the archive. Nightly ones are in `ANYPLAY_BACKUP_DIR` (default `cloud/deploy/backups`), named `anyplay-cloud-<UTC time>.tar.gz` with a `.sha256` next to it. If the server's disk is gone, take the copy [`offsite.sh`](../deploy/offsite.sh) made (below). An update also leaves one taken right before the new version started.
3. Note the commits you are on (`git -C cloud rev-parse HEAD`). An archive made by a newer version may hold a migration the older code does not know.

## 1. Prove the archive (touches nothing real)

```sh
cd cloud/deploy
./restore.sh backups/anyplay-cloud-20261027T033000Z.tar.gz
```

Expected: `manifest ok`, `integrity_check=ok accounts=<n>` and the wall-clock time. `<n>` should be close to the account count you expect. It exits non-zero on any failure; then take an older archive. `docker volume rm anyplay_restore-test` removes the scratch copy.

## 2. Restore for real

```sh
./restore.sh backups/anyplay-cloud-20261027T033000Z.tar.gz --really
```

It repeats step 1, then stops the whole stack, saves the current volume as `anyplay-cloud-pre-restore-<time>.tar.gz` next to the archive (this is your way back), replaces the database, `signing.key` and `ban.key`, checks the database again, **deletes again the accounts deleted since the archive** (the privacy notice promises it) and starts the stack. If a step fails after the stop, it puts the old volume back and starts the old stack. Screenshots and clips (`media/`) come back with the same archive; restore both from one archive or media messages will not open.

If it says the image does not know `replay-deletions`, run it by hand before players come back:

```sh
docker compose run --rm -T --no-deps cloud replay-deletions --ledger /dev/stdin < backups/anyplay-cloud-pre-restore-<time>.deletions
```

## 3. Check

```sh
docker compose ps                                  # cloud, relay, caddy (and web) healthy
curl -fsS https://anyplaynet.com/v1/status         # {"status":"operational", ...}
curl -fsS https://anyplaynet.com/v1/info
```

Then sign in with the launcher or the website as a real account and open the friends list. Write the time from the stop to the sign-in into the table above.

## If the whole server is gone

1. A new machine, Docker and the repositories as in [deploy.md](deploy.md); the same `deploy/.env` (keep a copy off the server; it is not in the backups).
2. Copy the newest archive from the off-server target to `deploy/backups/` (decrypt it first if you used `ANYPLAY_OFFSITE_AGE_RECIPIENT`: `age -d -i key.txt -o anyplay-cloud-<time>.tar.gz anyplay-cloud-<time>.tar.gz.age`; the manifest is only made for unencrypted copies, `restore.sh` then needs one: `sha256sum anyplay-cloud-<time>.tar.gz > anyplay-cloud-<time>.tar.gz.sha256`).
3. Run `update.sh` once with `--no-backup` so the stack and the volume exist, then `./restore.sh <archive> --really`.
4. Point the DNS at the new machine.

The signing key is in the archive. Without it every token and relay ticket is invalid and players must sign in again.

## Backups: what runs, and where the copies go

- **Before every update:** `update.sh` takes a backup right before the new version starts.
- **Nightly (install once, as code in `deploy/systemd/`):** `anyplay-backup.timer` runs `backup.sh --online`: a consistent snapshot of the database without stopping the cloud, compressed, mode 0600, with a manifest. It keeps the newest 14 and none older than 90 days.
- **Off the server (needs a decision, no purchase):** `offsite.sh` copies the newest archive to a machine you already own, by rsync over SSH (`ANYPLAY_OFFSITE_TARGET=user@host:/path`) or any rclone remote (`rclone:remote:path`), checks the copy, and prunes old ones at the target (rsync). With `ANYPLAY_OFFSITE_AGE_RECIPIENT` set to an [age](https://age-encryption.org) public key the archive is encrypted before it leaves; keep the private key elsewhere. Uncomment `ExecStartPost` in `anyplay-backup.service` to run it after every nightly backup. The operator has to choose the target and create a restricted SSH key for it; a backup on the same disk as the data is not a backup.
