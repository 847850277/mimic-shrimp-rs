#!/usr/bin/env bash

if [ -z "${BASH_VERSION:-}" ]; then
  exec bash "$0" "$@"
fi

set -Eeuo pipefail

INVOCATION_DIR="$(pwd)"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

CONTAINER_NAME="${CONTAINER_NAME:-mimic-shrimp-rs}"
IMAGE_NAME="${IMAGE_NAME:-mimic-shrimp-rs:latest}"
ENV_FILE="${ENV_FILE:-.env}"
REMOTE_NAME="${REMOTE_NAME:-origin}"
BRANCH_NAME="${BRANCH_NAME:-$(git rev-parse --abbrev-ref HEAD)}"
HOST_PORT="${HOST_PORT:-7878}"
CONTAINER_PORT="${CONTAINER_PORT:-7878}"
DOCKER_BUILD_PLATFORM="${DOCKER_BUILD_PLATFORM:-linux/amd64}"
BUILDER_IMAGE="${BUILDER_IMAGE:-docker.m.daocloud.io/library/rust:1.89-bookworm}"
RUNTIME_IMAGE="${RUNTIME_IMAGE:-docker.m.daocloud.io/library/debian:bookworm-slim}"
APP_FEATURES="${APP_FEATURES:-}"

log() {
  printf '[deploy] %s\n' "$*"
}

fail() {
  printf '[deploy] error: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

resolve_env_file() {
  local candidate="$ENV_FILE"
  local -a search_paths=()

  if [[ "$candidate" == /* ]]; then
    [[ -f "$candidate" ]] || fail "env file not found: $candidate"
    ENV_FILE="$candidate"
    return
  fi

  search_paths=(
    "$ROOT_DIR/$candidate"
    "$INVOCATION_DIR/$candidate"
    "$SCRIPT_DIR/$candidate"
  )

  local path=""
  for path in "${search_paths[@]}"; do
    if [[ -f "$path" ]]; then
      ENV_FILE="$path"
      return
    fi
  done

  fail "env file not found: $candidate"
}

assert_clean_worktree() {
  git diff --quiet || fail "git worktree has unstaged changes; commit or stash them before deploy"
  git diff --cached --quiet || fail "git worktree has staged changes; commit or stash them before deploy"
}

build_image() {
  local -a build_cmd=(
    docker build
    --platform "$DOCKER_BUILD_PLATFORM"
    --build-arg "BUILDER_IMAGE=$BUILDER_IMAGE"
    --build-arg "RUNTIME_IMAGE=$RUNTIME_IMAGE"
    -t "$IMAGE_NAME"
  )

  if [[ -n "$APP_FEATURES" ]]; then
    build_cmd+=(--build-arg "APP_FEATURES=$APP_FEATURES")
  fi

  build_cmd+=(.)

  log "building image $IMAGE_NAME"
  "${build_cmd[@]}"
}

restore_backup_container() {
  local backup_name="$1"

  if [[ -z "$backup_name" ]]; then
    return 0
  fi

  if docker ps -a --format '{{.Names}}' | grep -Fxq "$CONTAINER_NAME"; then
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
  fi

  if docker ps -a --format '{{.Names}}' | grep -Fxq "$backup_name"; then
    log "restoring previous container from $backup_name"
    docker rename "$backup_name" "$CONTAINER_NAME"
    docker start "$CONTAINER_NAME" >/dev/null
  fi
}

start_new_container() {
  local -a run_cmd=(
    docker run -d
    --name "$CONTAINER_NAME"
    --restart unless-stopped
    -p "${HOST_PORT}:${CONTAINER_PORT}"
    --env-file "$ENV_FILE"
    -e "SERVER_ADDR=0.0.0.0:${CONTAINER_PORT}"
    "$IMAGE_NAME"
  )

  "${run_cmd[@]}"
}

main() {
  require_command git
  require_command docker

  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || fail "current directory is not a git repository"
  [[ "$BRANCH_NAME" != "HEAD" ]] || fail "detached HEAD is not supported; set BRANCH_NAME explicitly"
  resolve_env_file

  assert_clean_worktree

  log "pulling latest code from $REMOTE_NAME/$BRANCH_NAME"
  git fetch "$REMOTE_NAME" "$BRANCH_NAME"
  git pull --ff-only "$REMOTE_NAME" "$BRANCH_NAME"

  log "using env file $ENV_FILE"
  build_image

  local backup_name=""
  if docker ps -a --format '{{.Names}}' | grep -Fxq "$CONTAINER_NAME"; then
    backup_name="${CONTAINER_NAME}-backup-$(date +%Y%m%d%H%M%S)"
    log "preparing backup container $backup_name"
    docker rename "$CONTAINER_NAME" "$backup_name"
    docker stop "$backup_name" >/dev/null
  fi

  local container_id=""
  if ! container_id="$(start_new_container)"; then
    restore_backup_container "$backup_name"
    fail "failed to start new container"
  fi

  sleep 2
  if ! docker ps --format '{{.Names}}' | grep -Fxq "$CONTAINER_NAME"; then
    docker logs --tail 100 "$CONTAINER_NAME" || true
    restore_backup_container "$backup_name"
    fail "new container exited during startup"
  fi

  if [[ -n "$backup_name" ]] && docker ps -a --format '{{.Names}}' | grep -Fxq "$backup_name"; then
    docker rm -f "$backup_name" >/dev/null
  fi

  log "deploy succeeded, container_id=$container_id"
  docker logs --tail 20 "$CONTAINER_NAME" || true
}

main "$@"
