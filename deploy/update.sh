#!/usr/bin/env bash
# Updates a running AnyPlay cloud deployment (the compose file in this folder) to given commits of
# the cloud and network repositories, checks that it came up, and rolls back when it did not.
#
#   cloud/deploy/update.sh [--cloud REF] [--network REF] [--pre-hook CMD] [--timeout SECONDS]
#                          [--no-rollback] [--dry-run]
#
# REF is a commit, tag or branch (a branch means its state on `origin`). Both default to origin/main.
# Details, what "near-zero downtime" means and how to roll back by hand: docs/deploy.md.
set -Eeuo pipefail

# Everything lives in a function and runs from the last line: the checkout below replaces this
# very file, and bash reads a script while it runs.
main() {
  local cloud_ref=origin/main network_ref=origin/main hook=${ANYPLAY_UPDATE_HOOK:-} \
    timeout=${ANYPLAY_UPDATE_TIMEOUT:-180} rollback=1 dry=0
  while [ $# -gt 0 ]; do
    case $1 in
      --cloud) cloud_ref=${2:?--cloud needs a ref}; shift 2 ;;
      --network) network_ref=${2:?--network needs a ref}; shift 2 ;;
      --pre-hook) hook=${2:?--pre-hook needs a command}; shift 2 ;;
      --timeout) timeout=${2:?--timeout needs seconds}; shift 2 ;;
      --no-rollback) rollback=0; shift ;;
      --dry-run) dry=1; shift ;;
      -h|--help) sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; return 0 ;;
      *) echo "unknown argument: $1 (see --help)" >&2; return 2 ;;
    esac
  done

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
  [ -d "$cloud_dir/.git" ] && [ -d "$network_dir/.git" ] || die "expected git checkouts at $cloud_dir and $network_dir"
  [ -f "$deploy/.env" ] || die "$deploy/.env is missing (copy .env.example)"

  # One update at a time.
  exec 9>"${TMPDIR:-/tmp}/anyplay-update.lock"
  flock -n 9 || die "another update is running"

  cd "$deploy"
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
  network_new=$(resolve "$network_dir" "${network_ref#origin/}")
  cloud_old=$(git -C "$cloud_dir" rev-parse HEAD)
  network_old=$(git -C "$network_dir" rev-parse HEAD)
  log "cloud   ${cloud_old:0:10} -> ${cloud_new:0:10}"
  log "network ${network_old:0:10} -> ${network_new:0:10}"
  if [ "$dry" = 1 ]; then log "dry run: stopping here"; return 0; fi

  local free_gb
  free_gb=$(df -BG --output=avail "$root" | tail -1 | tr -dc 0-9)
  [ "${free_gb:-0}" -ge 5 ] || die "less than 5 GB free on $root"

  # 2. Backup (or any check) the operator wants before the first change.
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
  log "building"
  if ! docker compose build --quiet; then
    log "build failed; restoring the previous sources, the running version was not touched"
    restore_sources
    return 1
  fi

  # 5. Replace only the services whose image or settings changed. The cloud has one SQLite file and
  # so one writer: it stops and starts again in a few seconds, the relay and Caddy follow.
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
  log "data changes of a newer version (database migrations) are not undone; restore a backup for that"
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
