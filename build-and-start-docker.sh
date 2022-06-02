#!/usr/bin/env sh
# Full clippy-checked Docker build for local dev.
set -xe
docker-compose down
mkdir "workdir" || true
cargo clippy
cargo clippy --features discord
docker buildx build . -t spritecollab-srv
docker-compose up --no-log-prefix
