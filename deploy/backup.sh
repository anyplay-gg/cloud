#!/usr/bin/env bash
# Backs up the cloud-data volume (the SQLite database, signing.key and ban.key) into one archive.
#
#   cloud/deploy/backup.sh [--dir DIR] [--keep N] [--max-age-days D] [--volume NAME] [--no-stop | --online]
#
# The cloud container stops for the copy, so the SQLite write-ahead log is checkpointed and the
# archive is consistent, and starts again afterwards, also when the copy failed. The archive is
# written under a temporary name, synced and then renamed, so a partial file never looks like a
# backup. Next to it goes <archive>.sha256. Both are mode 0600. Afterwards only the newest N
# archives (default 14) stay, and none older than D days (default 90, the longest the privacy
# notice says deleted data can stay in a backup; the age is read from the archive's name).
# Exit status is non-zero when anything went wrong.
# Restoring and the restore drill: restore.sh. Details: docs/deploy.md.
#
#   --dir DIR       where the archives go (default $ANYPLAY_BACKUP_DIR, else deploy/backups)
#   --keep N        archives to keep (default $ANYPLAY_BACKUP_KEEP, else 14)
#   --max-age-days D  remove archives older than D days (default $ANYPLAY_BACKUP_RETAIN_DAYS, else 90)
#   --volume NAME   the volume to copy (default $ANYPLAY_CLOUD_VOLUME, else anyplay_cloud-data)
#   --no-stop       do not stop the cloud; only for a volume that nothing writes to (tests)
#   --online        do not stop the cloud: snapshot.py copies the database with SQLite's online
#                   backup API (a consistent state while the cloud keeps writing, checked with
#                   integrity_check), no downtime. Same archive layout, restore.sh takes it as is.
#
# The copy runs in a throwaway container of $ANYPLAY_BACKUP_IMAGE (default python:3.13-alpine,
# which has tar and sha256sum, pinned by digest); the cloud image needs no extra tool. The image is
# fetched before the cloud stops, so a slow or failing registry never keeps the cloud down.
set -Eeuo pipefail

main() {
  local here dir=${ANYPLAY_BACKUP_DIR:-} keep=${ANYPLAY_BACKUP_KEEP:-14} retain=${ANYPLAY_BACKUP_RETAIN_DAYS:-90} \
    volume=${ANYPLAY_CLOUD_VOLUME:-anyplay_cloud-data} stop=1 online=0 \
    image=${ANYPLAY_BACKUP_IMAGE:-python:3.13-alpine@sha256:bf12ecc0bea3f171d55145dd02c78b5eeb09f30ee2ac9cc2b4c9e4e8f5e30741}
  wait=${ANYPLAY_BACKUP_TIMEOUT:-120} # wait, tmp, stopped: global, the EXIT trap runs after main has returned
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  # shellcheck source=backup-env.sh
  . "$here/backup-env.sh"
  anyplay_backup_env "$here"
  dir=${dir:-${ANYPLAY_BACKUP_DIR:-}} keep=${ANYPLAY_BACKUP_KEEP:-$keep} retain=${ANYPLAY_BACKUP_RETAIN_DAYS:-$retain} volume=${ANYPLAY_CLOUD_VOLUME:-$volume} \
    image=${ANYPLAY_BACKUP_IMAGE:-$image}
  while [ $# -gt 0 ]; do
    case $1 in
      --dir) dir=${2:?--dir needs a folder}; shift 2 ;;
      --keep) keep=${2:?--keep needs a number}; shift 2 ;;
      --max-age-days) retain=${2:?--max-age-days needs a number}; shift 2 ;;
      --volume) volume=${2:?--volume needs a name}; shift 2 ;;
      --no-stop) stop=0; shift ;;
      --online) stop=0; online=1; shift ;;
      -h|--help) sed -n '2,26p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; return 0 ;;
      *) echo "unknown argument: $1 (see --help)" >&2; return 2 ;;
    esac
  done
  [ -n "$dir" ] || dir=$here/backups
  case $keep in '' | *[!0-9]* | 0) echo "--keep must be a number of at least 1" >&2; return 2 ;; esac
  case $retain in '' | *[!0-9]* | 0) echo "--max-age-days must be a number of at least 1" >&2; return 2 ;; esac

  log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*"; }
  die() { log "ERROR: $*" >&2; exit 1; }

  command -v docker >/dev/null || die "docker is not installed"
  command -v sha256sum >/dev/null || die "sha256sum is not installed"
  docker volume inspect "$volume" >/dev/null 2>&1 || die "volume $volume does not exist"

  # Everything that can fail cheaply fails before the cloud stops.
  umask 077
  mkdir -p "$dir" || die "cannot create $dir"
  dir=$(cd "$dir" && { pwd -W 2>/dev/null || pwd; }) # -W: a Windows path for Docker Desktop (Git Bash)
  local probe=$dir/.write-test.$$
  { : >"$probe" && rm -f "${probe:?}"; } 2>/dev/null || die "$dir is not writable"
  # The image must be here before the cloud stops: a pull takes the cloud down for as long as it lasts, and a
  # registry failure or rate limit would block every update with the cloud stopped.
  docker image inspect "$image" >/dev/null 2>&1 || docker pull --quiet "$image" >/dev/null || die "cannot get the image $image; nothing was changed"

  local stamp name
  stamp=$(date -u +%Y%m%dT%H%M%SZ)
  name=anyplay-cloud-$stamp.tar.gz
  tmp=$dir/.$name.partial
  [ ! -e "$dir/$name" ] || die "$dir/$name exists; wait a second and run again"

  cd "$here"
  stopped=0
  restart_cloud() {
    [ "$stopped" = 1 ] || return 0
    stopped=0
    log "starting the cloud"
    docker compose start cloud >/dev/null || return 1
    local cid
    cid=$(docker compose ps -q cloud)
    local i status=''
    for ((i = 0; i < wait; i++)); do
      status=$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$cid" 2>/dev/null || true)
      case $status in healthy | running) log "the cloud is $status again"; return 0 ;; esac
      sleep 1
    done
    log "the cloud is not healthy after ${wait}s (status: ${status:-unknown})" >&2
    return 1
  }
  finish() { # runs on every exit, so the cloud comes back whatever happened
    local rc=$?
    trap - EXIT
    rm -f "${tmp:?}"
    restart_cloud || rc=1
    exit "$rc"
  }
  trap finish EXIT
  trap 'exit 130' INT TERM

  local t0=$SECONDS
  if [ "$stop" = 1 ]; then
    if [ -n "$(docker compose ps -q --status running cloud 2>/dev/null)" ]; then
      log "stopping the cloud for a consistent copy"
      stopped=1
      docker compose stop cloud >/dev/null || die "cannot stop the cloud"
    else
      log "the cloud is not running; copying as it is and leaving it stopped"
    fi
  fi

  log "copying volume $volume"
  if [ "$online" = 1 ]; then
    # The volume is mounted writable because a reader of a WAL database must be able to use its -shm file; snapshot.py
    # only reads the database, through SQLite's backup API.
    docker run --rm --network none -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)"       -v "$volume":/data -v "$dir":/backup -v "$here/snapshot.py":/snapshot.py:ro "$image" sh -c '
        set -e
        python3 /snapshot.py /data "/backup/$1" >/dev/null
        chown "$HOST_UID:$HOST_GID" "/backup/$1" "/backup/$1.sha256"
        sync
      ' sh "$name" || die "the online snapshot failed; nothing was kept"
    [ -s "$dir/$name" ] || die "the archive is empty"
  else
    docker run --rm --network none -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)"       -v "$volume":/data:ro -v "$dir":/backup "$image" sh -c '
        set -e
        tar -czf "/backup/$1" -C /data .
        chown "$HOST_UID:$HOST_GID" "/backup/$1"
        chmod 0600 "/backup/$1"
        sync
      ' sh ".$name.partial" || die "the copy failed; nothing was kept"
    [ -s "$tmp" ] || die "the archive is empty"
    mv "$tmp" "$dir/$name"
  fi
  local did_stop=$stopped
  restart_cloud || die "the cloud did not come back; look at 'docker compose logs cloud'"
  [ "$did_stop" = 0 ] || log "the cloud was down for about $((SECONDS - t0))s, from the stop until its health check passed again"

  # The manifest: standard sha256sum format, so `sha256sum -c` verifies it.
  (cd "$dir" && sha256sum "$name" >"$name.sha256") || die "cannot write the manifest"
  chmod 0600 "$dir/$name.sha256"
  log "wrote $dir/$name ($(du -h "$dir/$name" | cut -f1))"

  # Retention: the newest $keep archives stay; the names sort by time.
  local old
  while IFS= read -r old; do
    [ -n "$old" ] || continue
    log "removing old backup $old"
    rm -f "${dir:?}/${old:?}" "${dir:?}/${old:?}.sha256"
  done < <(find "$dir" -maxdepth 1 -type f -printf '%f\n' | grep -E '^anyplay-cloud-[0-9]{8}T[0-9]{6}Z\.tar\.gz$' | sort -r | tail -n +"$((keep + 1))")
  # Retention by age: a count alone keeps archives for as long as no new one is made, and the
  # privacy notice says deleted data leaves the backups after $retain days. The names are UTC
  # times of a fixed width, so they compare as text; the archive just written is never old.
  local cutoff stamp_old
  cutoff=$(date -u -d "$retain days ago" +%Y%m%dT%H%M%SZ) || die "cannot work out the age limit"
  while IFS= read -r old; do
    [ -n "$old" ] || continue
    [ "$old" != "$name" ] || continue
    stamp_old=${old#anyplay-cloud-}
    stamp_old=${stamp_old%.tar.gz}
    [[ "$stamp_old" < "$cutoff" ]] || continue
    log "removing backup $old, older than $retain days"
    rm -f "${dir:?}/${old:?}" "${dir:?}/${old:?}.sha256"
  done < <(find "$dir" -maxdepth 1 -type f -printf '%f\n' | grep -E '^anyplay-cloud-[0-9]{8}T[0-9]{6}Z\.tar\.gz$' | sort)
  printf '%s\n' "$dir/$name"
}

main "$@"
