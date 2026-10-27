#!/usr/bin/env bash
# Restores a backup made by backup.sh. By default it only proves the archive is good: it unpacks
# into a scratch volume and checks the database there; the running cloud is not touched.
#
#   cloud/deploy/restore.sh ARCHIVE [--volume NAME] [--really]
#
#   --volume NAME   the volume to unpack into (default anyplay_restore-test, emptied first)
#   --really        restore into the real volume ($ANYPLAY_CLOUD_VOLUME, else anyplay_cloud-data).
#                   The archive is first unpacked and checked in a scratch volume; only then the
#                   whole stack stops, the current volume is saved next to the archive as
#                   anyplay-cloud-pre-restore-<time>.tar.gz, and its contents (database,
#                   signing.key, ban.key) are replaced. When a step fails, the old volume is
#                   put back and the old stack starts again.
#                   Accounts deleted since the archive was made come back with it. Their IDs (and
#                   nothing else) are read from the deletion ledger of the volume that is
#                   replaced, saved next to the safety copy as ...pre-restore-<time>.deletions,
#                   and deleted again with `anyplay-cloud replay-deletions` before the stack
#                   starts. An image without that command only gets a warning.
#                   Safety copies older than $ANYPLAY_PRE_RESTORE_DAYS (default 90) days are
#                   deleted at the end: the privacy notice promises deleted data leaves backups
#                   within 90 days.
#
# The check: the SHA256 manifest next to the archive, then PRAGMA integrity_check and the number
# of rows in accounts, run in a throwaway container ($ANYPLAY_BACKUP_IMAGE, default
# python:3.13-alpine pinned by digest, which carries SQLite). The wall-clock time is printed (the restore time
# of docs/deploy.md). Exit status is non-zero on any failure.
set -Eeuo pipefail

main() {
  local here archive='' volume='' really=0 real_volume=${ANYPLAY_CLOUD_VOLUME:-anyplay_cloud-data} \
    image=${ANYPLAY_BACKUP_IMAGE:-python:3.13-alpine@sha256:bf12ecc0bea3f171d55145dd02c78b5eeb09f30ee2ac9cc2b4c9e4e8f5e30741} wait=${ANYPLAY_BACKUP_TIMEOUT:-120}
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  # shellcheck source=backup-env.sh
  . "$here/backup-env.sh"
  anyplay_backup_env "$here"
  real_volume=${ANYPLAY_CLOUD_VOLUME:-$real_volume} image=${ANYPLAY_BACKUP_IMAGE:-$image}
  while [ $# -gt 0 ]; do
    case $1 in
      --volume) volume=${2:?--volume needs a name}; shift 2 ;;
      --really) really=1; shift ;;
      -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; return 0 ;;
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

  # The image is fetched before anything is stopped: a failing registry must not leave the stack down.
  docker image inspect "$image" >/dev/null 2>&1 || docker pull --quiet "$image" >/dev/null || die "cannot get the image $image; nothing was changed"

  # Emptied and filled in one container, so the volume never holds a mix of old and new files.
  unpack() { # volume folder file
    docker volume create "$1" >/dev/null || return 1
    docker run --rm --network none -v "$1":/data -v "$2":/backup:ro "$image" sh -c '
      set -e
      find /data -mindepth 1 -delete
      tar -xzf "/backup/$1" -C /data
      chown -R 10001:10001 /data
    ' sh "$3"
  }
  check_db() { # volume; prints one line, fails when the database is damaged
    # As the cloud's own user and read-write: SQLite needs to open the write-ahead log, and files it makes must not be root's.
    docker run --rm --network none --user 10001:10001 -v "$1":/data "$image" python3 -c '
import sqlite3, sys
con = sqlite3.connect("/data/anyplay-cloud.db")
check = [r[0] for r in con.execute("PRAGMA integrity_check")]
n = con.execute("SELECT count(*) FROM accounts").fetchone()[0]
print("integrity_check=" + ",".join(check) + " accounts=" + str(n))
sys.exit(0 if check == ["ok"] else 1)
'
  }

  # The archive is proven first, in a scratch volume (with --really too): the real volume is not
  # touched until the same unpack and the same database check have worked once.
  local scratch=$volume out f
  if [ "$really" = 1 ]; then scratch=anyplay_restore-test; fi
  log "unpacking into $scratch"
  unpack "$scratch" "$dir" "$base" || die "unpacking failed; nothing real was touched"
  log "checking the database"
  out=$(check_db "$scratch") || { log "${out:-no output}" >&2; die "the database in the archive is damaged; nothing real was touched"; }
  log "$out"
  for f in signing.key ban.key; do
    docker run --rm --network none -v "$scratch":/data:ro "$image" test -s "/data/$f" || log "note: $f is not in the archive"
  done

  if [ "$really" = 0 ]; then
    log "drill ok: $base restored into scratch volume $scratch in $((SECONDS - t0))s; nothing real was touched"
    return 0
  fi

  cd "$here"
  local safety='' emptied=0 ledger=''
  # From the first stop on, a failing step puts the old state back and starts the old stack again.
  fail_back() { # message
    log "ERROR: $1" >&2
    if [ "$emptied" = 1 ]; then
      if [ -n "$safety" ] && unpack "$volume" "$dir" "$safety"; then
        log "the volume is back as it was before the restore"
      else
        log "ERROR: could not put the old volume back; the safety copy is $dir/$safety" >&2
      fi
    fi
    log "starting the previous stack again"
    docker compose up -d --no-build --wait --wait-timeout "$wait" >/dev/null || log "ERROR: the stack did not come up; look at 'docker compose ps'" >&2
    exit 1
  }

  log "stopping the stack"
  docker compose stop >/dev/null || fail_back "cannot stop the stack"
  # A copy of the volume as it is now, so a restore that goes wrong, or turns out to be the wrong
  # archive, can be undone. backup.sh's retention does not match this name, so it stays until you delete it.
  safety=anyplay-cloud-pre-restore-$(date -u +%Y%m%dT%H%M%SZ).tar.gz
  log "saving the current volume as $dir/$safety"
  (umask 077 && docker run --rm --network none -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    -v "$volume":/data:ro -v "$dir":/backup "$image" sh -c '
      set -e
      tar -czf "/backup/$1" -C /data .
      chown "$HOST_UID:$HOST_GID" "/backup/$1"
      chmod 0600 "/backup/$1"
      sync
    ' sh "$safety") || { safety=''; fail_back "cannot save the current volume; it was not changed"; }
  (cd "$dir" && sha256sum "$safety" >"$safety.sha256" && chmod 0600 "$safety.sha256") || true

  # The deletion ledger of the volume that is about to be replaced: the IDs of the accounts
  # deleted so far, one "<id> <ms>" per line, no other data. The archive may be older than some
  # of these deletions, so they are replayed once the archive is in place.
  ledger=${safety%.tar.gz}.deletions
  if (umask 077 && docker run --rm --network none --user 10001:10001 -v "$volume":/data:ro "$image" python3 -c '
import sqlite3
con = sqlite3.connect("file:/data/anyplay-cloud.db?mode=ro&immutable=1", uri=True)
try:
    rows = con.execute("SELECT account_id, deleted_ms FROM deleted_accounts ORDER BY account_id").fetchall()
except sqlite3.OperationalError:
    rows = []  # a database from before the ledger
for r in rows:
    print(r[0], r[1])
' >"$dir/$ledger"); then
    log "saved the deletion ledger ($(wc -l <"$dir/$ledger" | tr -d ' ') entries) as $dir/$ledger"
  else
    rm -f "${dir:?}/${ledger:?}"
    ledger=''
    log "WARNING: could not read the deletion ledger of the current volume; accounts deleted since the archive will come back"
  fi

  emptied=1
  unpack "$volume" "$dir" "$base" || fail_back "unpacking into $volume failed"
  out=$(check_db "$volume") || { log "${out:-no output}" >&2; fail_back "the database in $volume is damaged after unpacking"; }
  log "$out"
  emptied=0
  # Delete again the accounts that were deleted after the archive was made. A failure (also an
  # image that does not know the command yet) does not undo the restore, but it is said loudly:
  # until it is run, those accounts exist again.
  log "deleting again the accounts deleted since the archive"
  if [ -n "$ledger" ] && docker compose run --rm -T --no-deps cloud replay-deletions --ledger /dev/stdin <"$dir/$ledger"; then
    log "deletions replayed"
  else
    log "WARNING: the deletions were NOT replayed. Accounts deleted since the archive exist again. Run, with the cloud stopped:" >&2
    log "WARNING:   docker compose run --rm -T --no-deps cloud replay-deletions --ledger /dev/stdin < $dir/${ledger:-<ledger file>}" >&2
  fi
  log "starting the stack"
  docker compose up -d --no-build --wait --wait-timeout "$wait" || die "the stack did not come up; look at 'docker compose ps' (the previous volume is in $dir/$safety)"
  log "restored into $volume in $((SECONDS - t0))s; the stack runs again; the volume as it was before is in $dir/$safety"

  # Safety copies older than the longest time deleted data may stay in a backup (the privacy
  # notice says 90 days). The age is read from the name, which sorts by time.
  local days=${ANYPLAY_PRE_RESTORE_DAYS:-90} cut old
  case $days in '' | *[!0-9]*) days=90 ;; esac
  if cut=$(date -u -d "$days days ago" +%Y%m%dT%H%M%SZ 2>/dev/null); then
    while IFS= read -r old; do
      [ -n "$old" ] || continue
      log "removing safety copy older than $days days: $old"
      rm -f "${dir:?}/${old:?}" "${dir:?}/${old:?}.sha256"
    done < <(find "$dir" -maxdepth 1 -type f -printf '%f\n' | grep -E '^anyplay-cloud-pre-restore-[0-9]{8}T[0-9]{6}Z\.(tar\.gz|deletions)$' | while read -r f; do t=${f#anyplay-cloud-pre-restore-}; [ "${t%%.*}" \< "$cut" ] && echo "$f"; done)
  else
    log "note: cannot compute the age of old safety copies (no GNU date); delete anyplay-cloud-pre-restore-* older than $days days by hand"
  fi
}

main "$@"
