#!/usr/bin/env bash
# Restores a backup made by backup.sh. By default it only proves the archive is good: it unpacks
# into a scratch volume and checks the database there; the running cloud is not touched.
#
#   cloud/deploy/restore.sh ARCHIVE [--volume NAME] [--really]
#
#   --volume NAME   the volume to unpack into (default anyplay_restore-test, emptied first)
#   --really        restore into the real volume ($ANYPLAY_CLOUD_VOLUME, else anyplay_cloud-data):
#                   stops the whole stack, replaces the volume's contents, starts it again.
#                   Replaces everything in the volume: the database, signing.key and ban.key.
#
# The check: the SHA256 manifest next to the archive, then PRAGMA integrity_check and the number
# of rows in accounts, run in a throwaway container ($ANYPLAY_BACKUP_IMAGE, default
# python:3.13-alpine, which carries SQLite). The wall-clock time is printed (the restore time
# of docs/deploy.md). Exit status is non-zero on any failure.
set -Eeuo pipefail

main() {
  local here archive='' volume='' really=0 real_volume=${ANYPLAY_CLOUD_VOLUME:-anyplay_cloud-data} \
    image=${ANYPLAY_BACKUP_IMAGE:-python:3.13-alpine} wait=${ANYPLAY_BACKUP_TIMEOUT:-120}
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  # shellcheck source=backup-env.sh
  . "$here/backup-env.sh"
  anyplay_backup_env "$here"
  real_volume=${ANYPLAY_CLOUD_VOLUME:-$real_volume} image=${ANYPLAY_BACKUP_IMAGE:-$image}
  while [ $# -gt 0 ]; do
    case $1 in
      --volume) volume=${2:?--volume needs a name}; shift 2 ;;
      --really) really=1; shift ;;
      -h|--help) sed -n '2,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; return 0 ;;
      -*) echo "unknown argument: $1 (see --help)" >&2; return 2 ;;
      *) [ -z "$archive" ] || { echo "one archive only" >&2; return 2; }; archive=$1; shift ;;
    esac
  done
  [ -n "$archive" ] || { echo "usage: restore.sh ARCHIVE [--volume NAME] [--really]" >&2; return 2; }

  log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*"; }
  die() { log "ERROR: $*" >&2; exit 1; }

  command -v docker >/dev/null || die "docker is not installed"
  command -v sha256sum >/dev/null || die "sha256sum is not installed"
  [ -f "$archive" ] || die "no such archive: $archive"
  local dir base
  dir=$(cd "$(dirname "$archive")" && { pwd -W 2>/dev/null || pwd; })
  base=$(basename "$archive")

  if [ "$really" = 1 ]; then
    [ -z "$volume" ] || [ "$volume" = "$real_volume" ] || die "--really restores into $real_volume; do not combine it with --volume"
    volume=$real_volume
  else
    volume=${volume:-anyplay_restore-test}
    [ "$volume" != "$real_volume" ] || die "$volume is the real volume; pass --really to restore into it"
  fi

  local t0=$SECONDS
  [ -f "$dir/$base.sha256" ] || die "no manifest $dir/$base.sha256"
  (cd "$dir" && sha256sum --check --status "$base.sha256") || die "the archive does not match its manifest"
  log "manifest ok"

  if [ "$really" = 1 ]; then
    cd "$here"
    log "stopping the stack"
    docker compose stop >/dev/null || die "cannot stop the stack"
  fi

  # Emptied and filled in one container, so the volume never holds a mix of old and new files.
  docker volume create "$volume" >/dev/null
  log "unpacking into $volume"
  docker run --rm --network none -v "$volume":/data -v "$dir":/backup:ro "$image" sh -c '
    set -e
    find /data -mindepth 1 -delete
    tar -xzf "/backup/$1" -C /data
    chown -R 10001:10001 /data
  ' sh "$base" || die "unpacking failed"

  log "checking the database"
  local out
  # As the cloud's own user and read-write: SQLite needs to open the write-ahead log, and files it makes must not be root's.
  out=$(docker run --rm --network none --user 10001:10001 -v "$volume":/data "$image" python3 -c '
import sqlite3, sys
con = sqlite3.connect("/data/anyplay-cloud.db")
check = [r[0] for r in con.execute("PRAGMA integrity_check")]
n = con.execute("SELECT count(*) FROM accounts").fetchone()[0]
print("integrity_check=" + ",".join(check) + " accounts=" + str(n))
sys.exit(0 if check == ["ok"] else 1)
') || { log "${out:-no output}" >&2; die "the database in the archive is damaged"; }
  log "$out"
  local f
  for f in signing.key ban.key; do
    docker run --rm --network none -v "$volume":/data:ro "$image" test -s "/data/$f" || log "note: $f is not in the archive"
  done

  if [ "$really" = 1 ]; then
    log "starting the stack"
    docker compose up -d --no-build --wait --wait-timeout "$wait" || die "the stack did not come up; look at 'docker compose ps'"
    log "restored into $volume in $((SECONDS - t0))s; the stack runs again"
  else
    log "drill ok: $base restored into scratch volume $volume in $((SECONDS - t0))s; nothing real was touched"
  fi
}

main "$@"
