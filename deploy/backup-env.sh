# shellcheck shell=bash
# Sourced by backup.sh, restore.sh and update.sh: the ANYPLAY_BACKUP_* and ANYPLAY_CLOUD_VOLUME
# settings may live in deploy/.env (next to the compose settings) instead of the environment.
# A value in the environment wins. Not meant to be run.
anyplay_backup_env() { # deploy-dir
  local v val file=$1/.env
  [ -f "$file" ] || return 0
  for v in ANYPLAY_BACKUP_DIR ANYPLAY_BACKUP_KEEP ANYPLAY_BACKUP_MAX_AGE ANYPLAY_BACKUP_IMAGE ANYPLAY_CLOUD_VOLUME; do
    [ -z "${!v:-}" ] || continue
    val=$(grep -E "^$v=" "$file" | tail -n 1 | cut -d= -f2- | tr -d '\r' | sed -e 's/^"\(.*\)"$/\1/' -e "s/^'\(.*\)'\$/\1/") || true
    [ -z "$val" ] || export "$v=$val"
  done
}
