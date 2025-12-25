#!/usr/bin/env bash
set -euo pipefail

export HOST_UID="${HOST_UID:-$(id -u)}"
export HOST_GID="${HOST_GID:-$(id -g)}"

docker compose up -d --build "$@"
