#!/usr/bin/env bash
# Updates a running AnyPlay cloud deployment (the compose file in this folder) to given commits of
# the cloud and network repositories, checks that it came up, and rolls back when it did not.
#
#   cloud/deploy/update.sh [--cloud REF] [--network REF] [--pre-hook CMD] [--timeout SECONDS]
#                          [--no-rollback] [--no-backup] [--dry-run]
#   cloud/deploy/update.sh --cloud COMMIT --image sha256:... --relay-image sha256:... [--web-image sha256:...]
#                          [--no-attestation] [the options above]
#
# REF is a commit, tag or branch (a branch means its state on `origin`). --cloud defaults to
# origin/main; --network defaults to the commit in `network.ref` of the cloud commit being deployed.
# After the build, right before the new version starts, it runs backup.sh and stops if that fails
# (--no-backup skips it).
# With --image the cloud and relay images are pulled from ghcr.io (built by CI, checked against their
# attestation) instead of built here; --web-image does the same for the website (anyplaynet/web).
# Caddy is still built here.
# Details, what "near-zero downtime" means and how to roll back by hand: docs/deploy.md.
set -Eeuo pipefail

# Everything lives in a function and runs from the last line: the checkout below replaces this
# very file, and bash reads a script while it runs.
main() {
  local cloud_ref=origin/main network_ref='' hook=${ANYPLAY_UPDATE_HOOK:-} \
    timeout=${ANYPLAY_UPDATE_TIMEOUT:-180} rollback=1 dry=0 backup=1 
    image='' relay_image='' web_image='' attest=1 registry=${ANYPLAY_IMAGE_REGISTRY:-ghcr.io/anyplaynet}
  while [ $# -gt 0 ]; do
    case $1 in
      --cloud) cloud_ref=${2:?--cloud needs a ref}; shift 2 ;;
      --network) network_ref=${2:?--network needs a ref}; shift 2 ;;
      --pre-hook) hook=${2:?--pre-hook needs a command}; shift 2 ;;
      --timeout) timeout=${2:?--timeout needs seconds}; shift 2 ;;
      --no-rollback) rollback=0; shift ;;
      --no-backup) backup=0; shift ;;
      --image) image=${2:?--image needs the digest of the cloud image (sha256:...)}; shift 2 ;;
      --relay-image) relay_image=${2:?--relay-image needs the digest of the relay image (sha256:...)}; shift 2 ;;
      --web-image) web_image=${2:?--web-image needs the digest of the web image (sha256:...)}; shift 2 ;;
      --no-attestation) attest=0; shift ;;
      --dry-run) dry=1; shift ;;
      -h|--help) sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; return 0 ;;
      *) echo "unknown argument: $1 (see --help)" >&2; return 2 ;;
    esac
  done

  # Image mode: both digests, written out in full, and the commit they were built from.
  if [ -n "$image$relay_image" ]; then
    local d
    for d in "$image" "$relay_image"; do
      [[ $d =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "--image and --relay-image each take a digest, sha256:<64 hex characters>" >&2; return 2; }
    done
    [ "$cloud_ref" != origin/main ] || { echo "--image needs --cloud <the commit the images were built from>" >&2; return 2; }
  fi
  if [ -n "$web_image" ]; then
    [[ $web_image =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "--web-image takes a digest, sha256:<64 hex characters>" >&2; return 2; }
  fi

  local here root deploy
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  deploy=$here
  root=$(cd "$here/../.." && pwd)
  local cloud_dir=$root/cloud network_dir=$root/network

  log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*"; }
  die() { log "ERROR: $*" >&2; exit 1; }

  command -v docker >/dev/null || die "docker is not installed"
  command -v git >/dev/null || die "git is not installed"
  command -v curl >/dev/null || die "curl is not installed"
  { [ -d "$cloud_dir/.git" ] && [ -d "$network_dir/.git" ]; } || die "expected git checkouts at $cloud_dir and $network_dir"
  [ -f "$deploy/.env" ] || die "$deploy/.env is missing (copy .env.example)"

  # One update at a time.
  exec 9>"${TMPDIR:-/tmp}/anyplay-update.lock"
  flock -n 9 || die "another update is running"

  cd "$deploy"
  # shellcheck source=backup-env.sh
  . "$deploy/backup-env.sh"
  anyplay_backup_env "$deploy"
  for d in "$cloud_dir" "$network_dir"; do
    git -C "$d" diff --quiet HEAD -- || die "$d has uncommitted changes to tracked files; commit or stash them"
  done

  # 1. Resolve the refs. Nothing has changed yet.
  resolve() { # dir ref -> commit id
    git -C "$1" fetch --quiet --tags --prune origin
    git -C "$1" rev-parse --verify --quiet "origin/$2^{commit}" \
      || git -C "$1" rev-parse --verify --quiet "$2^{commit}" \
      || die "cannot resolve '$2' in $1"
  }
  local cloud_new network_new cloud_old network_old
  cloud_new=$(resolve "$cloud_dir" "${cloud_ref#origin/}")
  if [ -z "$network_ref" ]; then # the pin that belongs to the cloud commit, not the one checked out now
    network_ref=$(git -C "$cloud_dir" show "$cloud_new:network.ref" 2>/dev/null | tr -d '[:space:]') \
      || die "no network.ref in cloud commit ${cloud_new:0:10}; pass --network REF"
    [ -n "$network_ref" ] || die "network.ref is empty in cloud commit ${cloud_new:0:10}; pass --network REF"
  fi
  network_new=$(resolve "$network_dir" "${network_ref#origin/}")
  cloud_old=$(git -C "$cloud_dir" rev-parse HEAD)
  network_old=$(git -C "$network_dir" rev-parse HEAD)
  log "cloud   ${cloud_old:0:10} -> ${cloud_new:0:10}"
  log "network ${network_old:0:10} -> ${network_new:0:10}"
  if [ "$dry" = 1 ]; then log "dry run: stopping here"; return 0; fi

  local free_gb
  free_gb=$(df -BG --output=avail "$root" | tail -1 | tr -dc 0-9)
  [ "${free_gb:-0}" -ge 5 ] || die "less than 5 GB free on $root"

  # 2. Check what the backup needs, now, while nothing has changed. The backup itself is taken in step 4,
  # right before the new version starts: the build takes minutes while the old version keeps serving and
  # writing, and a backup from before the build would lose everything written meanwhile. A new version may
  # migrate the database on its first start, and the old one does not undo that.
  local volume=${ANYPLAY_CLOUD_VOLUME:-anyplay_cloud-data} backup_dir backup_path='' max_age=${ANYPLAY_BACKUP_MAX_AGE:-900} do_backup=0
  backup_dir=${ANYPLAY_BACKUP_DIR:-$deploy/backups}
  case $max_age in '' | *[!0-9]*) die "ANYPLAY_BACKUP_MAX_AGE must be a number of seconds" ;; esac
  if [ "$backup" = 0 ]; then
    log "WARNING: --no-backup: no backup before this update; a migration cannot be undone"
  elif docker volume inspect "$volume" >/dev/null 2>&1; then
    do_backup=1
    { mkdir -p "$backup_dir" && [ -w "$backup_dir" ]; } 2>/dev/null || die "the backup folder $backup_dir is not writable; nothing was changed (--no-backup skips the backup)"
  else
    # Not finding the volume is only a first install when there is no cloud container either. The volume's
    # name depends on the compose project name (COMPOSE_PROJECT_NAME, -p), so a live install can be here.
    local existing
    existing=$(docker compose ps -a -q cloud 2>/dev/null) || die "cannot ask docker compose about the cloud container; nothing was changed"
    [ -z "$existing" ] || die "no volume $volume, but a cloud container exists; set ANYPLAY_CLOUD_VOLUME to its data volume (docker inspect shows it); nothing was changed (--no-backup skips the backup)"
    log "no volume $volume and no cloud container (first install): nothing to back up"
  fi

  # Anything else the operator wants checked or done before the first change.
  if [ -n "$hook" ]; then
    log "pre-update hook"
    bash -c "$hook" || die "the pre-update hook failed; nothing was changed"
  fi

  # 3. Remember the running images, so a rollback does not depend on the build cache. The compose
  # files of the running version may not resolve with the settings of the new one (COMPOSE_FILE
  # naming a file only the new checkout has); then there is nothing to remember and the update goes on.
  local images name id
  images=$(docker compose config --images 2>/dev/null) || {
    log "the running version's compose files do not resolve; no images remembered for a rollback"
    images=""
  }
  for name in $images; do
    id=$(docker image inspect --format '{{.Id}}' "$name" 2>/dev/null || true)
    [ -n "$id" ] && docker tag "$id" "anyplay-rollback/${name}:previous"
  done

  checkout() { # dir commit
    git -C "$1" checkout --quiet --detach "$2"
  }
  restore_sources() {
    checkout "$cloud_dir" "$cloud_old"
    checkout "$network_dir" "$network_old"
  }

  # 4. Build with the old version still serving. A failed build changes nothing.
  checkout "$network_dir" "$network_new"
  checkout "$cloud_dir" "$cloud_new"
  # Caddy reads its files through single-file bind mounts, which keep pointing at the old file after
  # a checkout; when they changed, only a new Caddy container sees them (a few seconds without TLS).
  local caddy_changed=0
  git -C "$cloud_dir" diff --quiet "$cloud_old" "$cloud_new" -- deploy/Caddyfile 'deploy/*.caddy' || caddy_changed=1
  fresh_caddy() {
    [ "$caddy_changed" = 0 ] && return 0
    log "Caddy's files changed; starting a new Caddy"
    docker compose up -d --no-build --no-deps --force-recreate --wait --wait-timeout "$timeout" caddy
  }
  # Pulled images are checked before anything is replaced: the pull itself, the commit the image says
  # it was built from (cloud and relay only: the website is built from its own repository, so it has
  # no cloud commit), and the attestation of the repository that built it.
  pull_checked() { # name repo digest expected_revision(or empty)
    local ref=$registry/$1@$3 rev
    if ! docker pull --quiet "$ref" >/dev/null; then
      log "cannot pull $ref (is the package public, or is docker logged in to ghcr.io?); restoring the previous sources, the running version was not touched"
      restore_sources
      return 1
    fi
    if [ -n "$4" ]; then
      rev=$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' "$ref")
      if [ "$rev" != "$4" ]; then
        log "$ref was built from ${rev:-no known commit}, not from $4; restoring the previous sources, the running version was not touched"
        restore_sources
        return 1
      fi
    fi
    if [ "$attest" = 0 ]; then
      log "WARNING: --no-attestation: $ref is not checked against its attestation"
    elif command -v gh >/dev/null; then
      if ! gh attestation verify "oci://$ref" --repo "anyplaynet/$2" >/dev/null; then
        log "no valid attestation for $ref (gh attestation verify failed; --no-attestation skips the check); restoring the previous sources, the running version was not touched"
        restore_sources
        return 1
      fi
      log "attestation ok: $ref"
    else
      log "WARNING: gh is not installed, so $ref is not checked against its attestation"
    fi
  }
  local skip=() others
  if [ -n "$image" ]; then
    log "pulling the cloud and relay images"
    pull_checked cloud cloud "$image" "$cloud_new" || return 1
    pull_checked relay cloud "$relay_image" "$cloud_new" || return 1
    skip+=(cloud relay)
  fi
  if [ -n "$web_image" ]; then
    docker compose config --services | grep -qx web       || { log "--web-image: the compose files have no web service (COMPOSE_FILE in .env needs compose.web.yaml); restoring the previous sources, the running version was not touched"; restore_sources; return 1; }
    log "pulling the web image"
    pull_checked web web "$web_image" '' || return 1
    skip+=(web)
  fi
  if [ ${#skip[@]} -gt 0 ]; then
    log "building the rest"
    others=$(docker compose config --services | grep -vxF "$(printf '%s
' "${skip[@]}")" | tr '
' ' ') || others=''
    # shellcheck disable=SC2086 # the service names are words
    if [ -n "$others" ] && ! docker compose build --quiet $others; then
      log "build failed; restoring the previous sources, the running version was not touched"
      restore_sources
      return 1
    fi
  else
    log "building"
    if ! docker compose build --quiet; then
      log "build failed; restoring the previous sources, the running version was not touched"
      restore_sources
      return 1
    fi
  fi

  # 4b. Back up now: the old version still runs, the new one is built and has not started, so the archive
  # holds everything written until this moment and the new version's migration comes after it. If the
  # backup fails, nothing is started and the old sources go back.
  if [ "$do_backup" = 1 ]; then
    log "backing up, right before the new version starts"
    if ! "$deploy/backup.sh" --dir "$backup_dir"; then
      log "the backup failed; restoring the previous sources, the running version was not touched (--no-backup skips the backup)"
      restore_sources
      return 1
    fi
    local newest age
    newest=$(find "$backup_dir" -maxdepth 1 -type f -printf '%f
' | grep -E '^anyplay-cloud-[0-9]{8}T[0-9]{6}Z[.]tar[.]gz$' | sort | tail -n 1) || true
    age=-1
    [ -z "$newest" ] || age=$(( $(date +%s) - $(stat -c %Y "$backup_dir/$newest") ))
    if [ -z "$newest" ] || [ "$age" -gt "$max_age" ]; then
      log "no fresh backup archive in $backup_dir (newest: ${newest:-none}); restoring the previous sources, the running version was not touched"
      restore_sources
      return 1
    fi
    backup_path=$backup_dir/$newest
    log "backup ok: $newest, ${age}s old; restore with deploy/restore.sh $backup_path"
  fi

  # 5. Replace only the services whose image or settings changed. The cloud has one SQLite file and
  # so one writer: it stops and starts again in a few seconds, the relay and Caddy follow.
  if [ -n "$image" ]; then
    # compose runs the names anyplay-cloud and anyplay-relay (compose.yaml); a manual
    # `docker compose up` later finds the same images. The rollback tags were made in step 3.
    docker tag "$registry/cloud@${image}" anyplay-cloud
    docker tag "$registry/relay@${relay_image}" anyplay-relay
  fi
  if [ -n "$web_image" ]; then
    docker tag "$registry/web@${web_image}" anyplay-web
  fi
  log "starting the new version"
  if docker compose up -d --no-build --remove-orphans --wait --wait-timeout "$timeout" \
     && fresh_caddy && verify "$timeout"; then
    log "update ok: cloud ${cloud_new:0:10}, network ${network_new:0:10}"
    docker image prune -f >/dev/null 2>&1 || true
    docker builder prune -f --filter until=168h >/dev/null 2>&1 || true
    return 0
  fi

  log "the new version did not come up"
  if [ "$rollback" = 0 ]; then log "--no-rollback: leaving it as it is"; return 1; fi
  log "rolling back to cloud ${cloud_old:0:10}, network ${network_old:0:10}"
  restore_sources
  for name in $images; do
    docker image inspect "anyplay-rollback/${name}:previous" >/dev/null 2>&1 \
      && docker tag "anyplay-rollback/${name}:previous" "$name"
  done
  docker compose up -d --no-build --remove-orphans --wait --wait-timeout "$timeout" || true
  fresh_caddy || true
  if verify "$timeout"; then
    log "rolled back; the previous version is serving again"
  else
    log "ROLLBACK FAILED: look at 'docker compose ps' and 'docker compose logs'"
  fi
  log "data changes of a newer version (database migrations) are not undone; restore the backup for that."
  log "Anything written after the backup (it was taken just before the new version started) is not in it:"
  if [ -n "$backup_path" ]; then
    log "  restore with deploy/restore.sh $backup_path --really   (without --really it only checks the archive)"
  else
    log "  restore with deploy/restore.sh <archive>; no backup was taken in this run"
  fi
  return 1
}

# The public address answers /v1/info (through Caddy and TLS) and nothing restarts in a loop.
verify() {
  local timeout=$1 domain url i body
  domain=$(grep -E '^CLOUD_DOMAIN=' .env | head -1 | cut -d= -f2- | tr -d '"' | tr -d "' \r")
  url=${ANYPLAY_UPDATE_URL:-https://$domain/v1/info}
  for ((i = 0; i < timeout / 3; i++)); do
    if body=$(curl -fsS --max-time 8 "$url" 2>/dev/null) && grep -q '"name"' <<<"$body"; then
      log "answers: $url"
      sleep 5
      if docker compose ps --format '{{.Name}} {{.State}}' | grep -vq ' running$'; then
        log "a container is not running"; docker compose ps; return 1
      fi
      return 0
    fi
    sleep 3
  done
  log "no answer from $url within ${timeout}s"
  return 1
}

main "$@"
exit $?
