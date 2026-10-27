#!/usr/bin/env bash
# Copies the newest backup archive off the server, to a machine you already own. No paid service.
#
#   cloud/deploy/offsite.sh [--dir DIR] [--target TARGET] [--keep N]
#
#   --dir DIR       where backup.sh puts archives (default $ANYPLAY_BACKUP_DIR, else deploy/backups)
#   --target T      where to copy: USER@HOST:/path (rsync over SSH) or rclone:REMOTE:path (any rclone
#                   remote you set up yourself). Default $ANYPLAY_OFFSITE_TARGET. Required.
#   --keep N        archives kept at the target (default $ANYPLAY_OFFSITE_KEEP, else 30). Only
#                   rsync targets are pruned, and only names that look like backup archives.
#
# Optional encryption before the archive leaves the server: set ANYPLAY_OFFSITE_AGE_RECIPIENT to an
# `age` public key (https://age-encryption.org) and the archive is copied as <name>.age. Keep the private
# key somewhere else than the server and the backups: without it the copy cannot be read. The archives
# hold signing.key and personal data, so the target needs the same care as the server (and the 90-day
# retention of the privacy notice: --keep 30 with a nightly timer is about a month).
# The copy is verified: rsync with --checksum must report nothing left to transfer. The SHA-256
# manifest travels with the archive (unencrypted archives only). Exit status is non-zero on failure.
# An SSH key without a passphrase for the target, restricted to rsync, is the usual setup.
set -Eeuo pipefail

main() {
  local here dir=${ANYPLAY_BACKUP_DIR:-} target=${ANYPLAY_OFFSITE_TARGET:-} keep=${ANYPLAY_OFFSITE_KEEP:-30} \
    recipient=${ANYPLAY_OFFSITE_AGE_RECIPIENT:-}
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  # shellcheck source=backup-env.sh
  . "$here/backup-env.sh"
  anyplay_backup_env "$here"
  dir=${dir:-${ANYPLAY_BACKUP_DIR:-}}
  while [ $# -gt 0 ]; do
    case $1 in
      --dir) dir=${2:?--dir needs a folder}; shift 2 ;;
      --target) target=${2:?--target needs a destination}; shift 2 ;;
      --keep) keep=${2:?--keep needs a number}; shift 2 ;;
      -h|--help) sed -n '2,19p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; return 0 ;;
      *) echo "unknown argument: $1 (see --help)" >&2; return 2 ;;
    esac
  done
  [ -n "$dir" ] || dir=$here/backups
  case $keep in '' | *[!0-9]* | 0) echo "--keep must be a number of at least 1" >&2; return 2 ;; esac
  log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*"; }
  die() { log "ERROR: $*" >&2; exit 1; }
  [ -n "$target" ] || die "no target: set ANYPLAY_OFFSITE_TARGET (USER@HOST:/path or rclone:REMOTE:path)"

  local newest name
  newest=$(find "$dir" -maxdepth 1 -type f -printf '%f\n' 2>/dev/null | grep -E '^anyplay-cloud-[0-9]{8}T[0-9]{6}Z\.tar\.gz$' | sort | tail -n 1 || true)
  [ -n "$newest" ] || die "no archive in $dir"
  name=$newest
  (cd "$dir" && sha256sum -c "$name.sha256" >/dev/null 2>&1) || die "$name does not match its manifest; not copying a damaged archive"

  local stage send
  stage=$(mktemp -d)
  trap 'rm -rf "${stage:?}"' EXIT
  if [ -n "$recipient" ]; then
    command -v age >/dev/null || die "ANYPLAY_OFFSITE_AGE_RECIPIENT is set but age is not installed"
    age -r "$recipient" -o "$stage/$name.age" "$dir/$name" || die "encryption failed"
    send=$stage
    set -- "$name.age"
  else
    cp "$dir/$name" "$dir/$name.sha256" "$stage/"
    send=$stage
    set -- "$name" "$name.sha256"
  fi

  case $target in
    rclone:*)
      command -v rclone >/dev/null || die "rclone is not installed"
      local remote=${target#rclone:} f
      for f in "$@"; do rclone copyto --checksum "$send/$f" "$remote/$f" || die "rclone failed"; done
      for f in "$@"; do rclone check --one-way "$send" "$remote" --include "$f" >/dev/null 2>&1 || die "the copy of $f does not verify"; done
      ;;
    *)
      command -v rsync >/dev/null || die "rsync is not installed"
      local f out
      for f in "$@"; do rsync -t --chmod=F600 "$send/$f" "$target/" || die "rsync failed"; done
      for f in "$@"; do
        out=$(rsync -n --checksum --itemize-changes "$send/$f" "$target/") || die "cannot verify the copy"
        [ -z "$out" ] || die "the copy of $f differs from the source"
      done
      # Retention at the target: the newest $keep archives (names sort by time).
      local host=${target%%:*} path=${target#*:}
      case $target in *:*) ;; *) host='' ;; esac
      if [ -n "$host" ]; then
        # shellcheck disable=SC2029
        ssh "$host" "cd '$path' && ls -1 | grep -E '^anyplay-cloud-[0-9]{8}T[0-9]{6}Z\.tar\.gz(\.age|\.sha256)?\$' | sed -E 's/\.(age|sha256)\$//' | sort -u | sort -r | tail -n +$((keep + 1)) | while read -r old; do rm -f -- \"\$old\" \"\$old.sha256\" \"\$old.age\"; done" ||
          log "warning: could not prune old archives at the target"
      else
        log "local target: not pruned"
      fi
      ;;
  esac
  log "copied $name off the server to $target${recipient:+ (encrypted)}"
}

main "$@"
