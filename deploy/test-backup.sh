#!/usr/bin/env bash
# Tests backup.sh, restore.sh and the backup step of update.sh on throwaway volumes. Needs a
# running Docker daemon; touches nothing real (no compose project, no anyplay_* volume).
#
#   cloud/deploy/test-backup.sh
#
# 1. A volume with a seeded SQLite database (WAL mode), signing.key and ban.key is backed up;
#    the archive and manifest are mode 0600; a later change to the volume is not in the archive.
# 2. The archive is restored into a scratch volume: integrity_check ok and the seeded row count.
# 3. A damaged archive and a missing manifest are refused; --keep and --max-age-days prune; an unwritable backup
#    directory fails before anything is touched.
# 4. update.sh, run against stand-in repositories and a stand-in docker: with an unwritable backup
#    directory it exits non-zero before `docker compose build`; with --no-backup it goes on to
#    build (and warns); --dry-run prints no backup step.
# 5. With a container standing in for the cloud: it is stopped before the copy, started after it,
#    and is running again also when the copy fails.
set -Eeuo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
image=${ANYPLAY_BACKUP_IMAGE:-python:3.13-alpine@sha256:bf12ecc0bea3f171d55145dd02c78b5eeb09f30ee2ac9cc2b4c9e4e8f5e30741}
id=$$
vol=anyplay_backuptest_src_$id
scratch=anyplay_backuptest_scratch_$id
work=$(mktemp -d)
work=$(cd "$work" && { pwd -W 2>/dev/null || pwd; }) # pwd -W: a path Docker Desktop on Windows can mount
failures=0

cleanup() {
  docker volume rm -f "$vol" "$scratch" >/dev/null 2>&1 || true
  # The archives are ours (0600, our user); the work folder is a fresh mktemp -d.
  [ -n "${work:-}" ] && rm -rf "${work:?}"
}
trap cleanup EXIT

ok() { printf 'ok   %s\n' "$*"; }
fail() { printf 'FAIL %s\n' "$*" >&2; failures=$((failures + 1)); }
check() { # description, command...
  local what=$1; shift
  if "$@" >/dev/null 2>&1; then ok "$what"; else fail "$what"; fi
}
show() { local l; while IFS= read -r l; do printf '     | %s\n' "$l"; done <<<"$1"; }
refuses() { # description, command...
  local what=$1; shift
  if "$@" >/dev/null 2>&1; then fail "$what (it succeeded)"; else ok "$what"; fi
}

# --- 1. seed and back up -------------------------------------------------------------------
docker volume create "$vol" >/dev/null
docker run --rm -v "$vol":/data "$image" sh -c '
  python3 - <<PY
import sqlite3
con = sqlite3.connect("/data/anyplay-cloud.db")
con.execute("PRAGMA journal_mode=WAL")
con.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, handle TEXT)")
con.executemany("INSERT INTO accounts (handle) VALUES (?)", [("a",), ("b",), ("c",)])
con.commit()
con.close()
PY
  echo signing-key > /data/signing.key
  echo ban-key > /data/ban.key
'
export ANYPLAY_BACKUP_IMAGE=$image
dir=$work/backups

archive=$("$here/backup.sh" --no-stop --volume "$vol" --dir "$dir" | tail -n 1)
check "backup wrote an archive" test -s "$archive"
check "archive is mode 0600" test "$(stat -c %a "$archive")" = 600
check "manifest is mode 0600" test "$(stat -c %a "$archive.sha256")" = 600
manifest_ok() { (cd "$1" && sha256sum --check --status "$2"); }
check "manifest verifies" manifest_ok "$dir" "$(basename "$archive").sha256"
check "no partial file is left" test -z "$(find "$dir" -name '*partial*')"

# --- 2. mutate, restore to scratch, assert ------------------------------------------------
docker run --rm -v "$vol":/data "$image" python3 -c '
import sqlite3
con = sqlite3.connect("/data/anyplay-cloud.db")
con.execute("DELETE FROM accounts")
con.commit()
con.close()
'
if out=$("$here/restore.sh" "$archive" --volume "$scratch" 2>&1); then
  ok "restore into the scratch volume"
else
  fail "restore into the scratch volume"; echo "$out" >&2
fi
case $out in *integrity_check=ok*) ok "integrity_check ok" ;; *) fail "integrity_check ok" ;; esac
case $out in *accounts=3*) ok "the seeded 3 accounts are back (the volume itself has 0 now)" ;; *) fail "account count is 3"; echo "$out" >&2 ;; esac
check "signing.key and ban.key came back" docker run --rm -v "$scratch":/data:ro "$image" sh -c 'grep -q signing-key /data/signing.key && grep -q ban-key /data/ban.key'

# --- 3. refusals ----------------------------------------------------------------------------
bad=$dir/anyplay-cloud-20000101T000000Z.tar.gz
cp "$archive" "$bad"; cp "$archive.sha256" "$bad.sha256"
printf '%064d  %s\n' 0 "$(basename "$bad")" >"$bad.sha256"
refuses "a wrong manifest is refused" "$here/restore.sh" "$bad" --volume "$scratch"
rm -f "$bad.sha256"
refuses "a missing manifest is refused" "$here/restore.sh" "$bad" --volume "$scratch"
refuses "the real volume needs --really" "$here/restore.sh" "$archive" --volume anyplay_cloud-data
rm -f "$bad"

sleep 1; "$here/backup.sh" --no-stop --volume "$vol" --dir "$dir" --keep 2 >/dev/null
sleep 1; "$here/backup.sh" --no-stop --volume "$vol" --dir "$dir" --keep 2 >/dev/null
check "--keep 2 leaves 2 archives and 2 manifests" test "$(find "$dir" -type f | wc -l)" = 4
check "the oldest archive was the one removed" test ! -e "$archive"

# Age: with a generous count, an archive older than the limit still goes, a younger one stays,
# and so does a pre-restore copy (its own name, its own rule in restore.sh).
old_name=anyplay-cloud-$(date -u -d '91 days ago' +%Y%m%dT%H%M%SZ).tar.gz
young_name=anyplay-cloud-$(date -u -d '89 days ago' +%Y%m%dT%H%M%SZ).tar.gz
pre_name=anyplay-cloud-pre-restore-$(date -u -d '120 days ago' +%Y%m%dT%H%M%SZ).tar.gz
for f in "$old_name" "$young_name" "$pre_name"; do : >"$dir/$f"; : >"$dir/$f.sha256"; done
sleep 1; "$here/backup.sh" --no-stop --volume "$vol" --dir "$dir" --keep 50 >/dev/null
check "an archive older than 90 days is removed with its manifest" test ! -e "$dir/$old_name" -a ! -e "$dir/$old_name.sha256"
check "an archive younger than 90 days stays" test -e "$dir/$young_name"
check "a pre-restore copy is not touched by the backup" test -e "$dir/$pre_name"
sleep 1; "$here/backup.sh" --no-stop --volume "$vol" --dir "$dir" --keep 50 --max-age-days 30 >/dev/null
check "--max-age-days 30 removes the 89-day-old archive" test ! -e "$dir/$young_name"
check "the archives just written stay" test "$(find "$dir" -name 'anyplay-cloud-2*.tar.gz' -size +0 | wc -l)" -ge 1
refuses "--max-age-days 0 is refused" "$here/backup.sh" --no-stop --volume "$vol" --dir "$dir" --max-age-days 0
rm -f "$dir/$pre_name" "$dir/$pre_name.sha256"

: >"$work/afile"
refuses "an unwritable backup directory fails" "$here/backup.sh" --no-stop --volume "$vol" --dir "$work/afile/sub"
refuses "a missing volume fails" "$here/backup.sh" --no-stop --volume anyplay_backuptest_nope_$id --dir "$dir"

# --- 4. update.sh against stand-ins ---------------------------------------------------------
# Real git repositories (so the ref logic runs), a stand-in docker that records what it is asked.
root=$work/root
mkdir -p "$root" "$work/src" "$work/bin"
git -C "$work/src" init -q -b main network
git -C "$work/src/network" -c user.name=t -c user.email=t@example.invalid commit -q --allow-empty -m n
net=$(git -C "$work/src/network" rev-parse HEAD)
git -C "$work/src" init -q -b main cloud
mkdir -p "$work/src/cloud/deploy"
cp "$here"/*.sh "$work/src/cloud/deploy/"
chmod +x "$work/src/cloud/deploy/"*.sh
printf '%s\n' "$net" >"$work/src/cloud/network.ref"
git -C "$work/src/cloud" add -A
git -C "$work/src/cloud" -c user.name=t -c user.email=t@example.invalid commit -q -m c
git clone -q "$work/src/cloud" "$root/cloud"
git clone -q "$work/src/network" "$root/network"
printf 'CLOUD_DOMAIN=cloud.example.invalid\n' >"$root/cloud/deploy/.env"

cat >"$work/bin/docker" <<'STUB'
#!/usr/bin/env bash
echo "docker $*" >>"$STUB_LOG"
case "$1 $2" in
  "volume inspect") exit "${STUB_VOLUME_RC:-0}" ;;
  "compose build") exit 0 ;;
  "compose config") exit 0 ;;
  "compose up") exit 0 ;;
  "compose ps") echo "${STUB_PS:-}"; exit 0 ;;
esac
exit 0
STUB
chmod +x "$work/bin/docker"
export STUB_LOG=$work/docker.log

run_update() { PATH=$work/bin:$PATH ANYPLAY_UPDATE_URL=http://127.0.0.1:9/none "$root/cloud/deploy/update.sh" --timeout 3 --no-rollback "$@"; }

if command -v flock >/dev/null && df -BG --output=avail "$root" >/dev/null 2>&1; then
  : >"$STUB_LOG"
  if out=$(ANYPLAY_BACKUP_DIR=$work/afile/sub run_update 2>&1); then
    fail "update.sh with an unwritable backup dir exits non-zero"
  else
    ok "update.sh with an unwritable backup dir exits non-zero"
  fi
  show "$out"
  check "...and says the folder is not writable" grep -q "is not writable" <<<"$out"
  refuses "...before docker compose build" grep -q "compose build" "$STUB_LOG"

  # A missing volume with a cloud container is no first install: stop before anything changes.
  : >"$STUB_LOG"
  if out=$(STUB_VOLUME_RC=1 STUB_PS=abc123 run_update 2>&1); then
    fail "update.sh with no volume but a cloud container exits non-zero"
  else
    ok "update.sh with no volume but a cloud container exits non-zero"
  fi
  check "...and asks for ANYPLAY_CLOUD_VOLUME" grep -q "ANYPLAY_CLOUD_VOLUME" <<<"$out"
  refuses "...before docker compose build" grep -q "compose build" "$STUB_LOG"
  : >"$STUB_LOG"
  out=$(STUB_VOLUME_RC=1 STUB_PS='' ANYPLAY_BACKUP_DIR=$work/b3 run_update 2>&1 || true)
  check "no volume and no container is a first install" grep -q "first install" <<<"$out"
  check "...and goes on to build" grep -q "compose build" "$STUB_LOG"

  : >"$STUB_LOG"
  out=$(run_update --no-backup 2>&1 || true)
  show "$out"
  check "--no-backup warns" grep -q "WARNING: --no-backup" <<<"$out"
  check "--no-backup goes on to build" grep -q "compose build" "$STUB_LOG"

  : >"$STUB_LOG"
  out=$(run_update --dry-run 2>&1)
  refuses "--dry-run mentions no backup" grep -qi backup <<<"$out"
  refuses "--dry-run calls no docker" test -s "$STUB_LOG"
else
  echo "skip update.sh stand-in tests: they need flock and GNU df (a Linux host)"
fi

# --- 5. the cloud container is stopped for the copy and always comes back --------------------
# A stand-in `docker compose` that stops and starts one real throwaway container; everything else
# goes to the real docker.
cid_name=anyplay_backuptest_cloud_$id
docker run -d --name "$cid_name" --health-cmd true --health-interval 1s -v "$vol":/data:ro "$image" sleep 600 >/dev/null
trap 'docker rm -f "$cid_name" >/dev/null 2>&1 || true; cleanup' EXIT
mkdir -p "$work/bin2"
cat >"$work/bin2/docker" <<'STUB'
#!/usr/bin/env bash
echo "docker $*" >>"$STUB_LOG"
if [ "$1" = compose ]; then
  case "$2" in
    ps) echo "$STUB_CID" ;;
    stop) "$REAL_DOCKER" stop "$STUB_CID" ;;
    start) "$REAL_DOCKER" start "$STUB_CID" ;;
  esac
  exit 0
fi
exec "$REAL_DOCKER" "$@"
STUB
chmod +x "$work/bin2/docker"
REAL_DOCKER=$(command -v docker)
STUB_CID=$(docker inspect --format '{{.Id}}' "$cid_name")
export REAL_DOCKER STUB_CID
hybrid() { PATH=$work/bin2:$PATH "$@"; }
running() { [ "$(docker inspect --format '{{.State.Running}}' "$cid_name")" = true ]; }
sleep 2

: >"$STUB_LOG"
t0=$SECONDS
hybrid "$here/backup.sh" --volume "$vol" --dir "$dir" --keep 5 >/dev/null
check "backup with the container: it is running again afterwards" running
stop_line=$(grep -n "compose stop cloud" "$STUB_LOG" | head -1 | cut -d: -f1)
run_line=$(grep -n "docker run" "$STUB_LOG" | head -1 | cut -d: -f1)
start_line=$(grep -n "compose start cloud" "$STUB_LOG" | head -1 | cut -d: -f1)
check "order: stop, copy, start" test "${stop_line:-9}" -lt "${run_line:-0}" -a "${run_line:-9}" -lt "${start_line:-0}"
echo "     | backup with stop and start took $((SECONDS - t0))s"

if ANYPLAY_BACKUP_IMAGE=anyplay.invalid/none:0 hybrid "$here/backup.sh" --volume "$vol" --dir "$dir" >/dev/null 2>&1; then
  fail "a failing copy exits non-zero"
else
  ok "a failing copy exits non-zero"
fi
check "...and the container is running again" running


if command -v flock >/dev/null && df -BG --output=avail "$root" >/dev/null 2>&1; then
  : >"$STUB_LOG"
  out=$(hybrid env ANYPLAY_CLOUD_VOLUME="$vol" ANYPLAY_BACKUP_DIR="$work/b2" ANYPLAY_UPDATE_URL=http://127.0.0.1:9/none \
    "$root/cloud/deploy/update.sh" --timeout 3 --no-rollback 2>&1 || true)
  show "$out"
  check "update.sh logs the backup and the archive age" grep -q "backup ok: anyplay-cloud-.*s old; restore with deploy/restore.sh" <<<"$out"
  build_line=$(grep -n "compose build" "$STUB_LOG" | head -1 | cut -d: -f1)
  stop_line=$(grep -n "compose stop cloud" "$STUB_LOG" | head -1 | cut -d: -f1)
  up_line=$(grep -n "compose up" "$STUB_LOG" | head -1 | cut -d: -f1)
  check "update.sh builds first, then backs up (stop), then starts the new version" test "${build_line:-9}" -lt "${stop_line:-0}" -a "${stop_line:-9}" -lt "${up_line:-0}"
  check "the archive is where update.sh said" test -n "$(find "$work/b2" -name 'anyplay-cloud-*.tar.gz')"
  check "the cloud container runs again after the update's backup" running
fi

if [ "$failures" -gt 0 ]; then echo "$failures check(s) failed" >&2; exit 1; fi
echo "all checks passed"
