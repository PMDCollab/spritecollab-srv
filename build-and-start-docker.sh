#!/usr/bin/env sh
# Full clippy-checked Docker build for local dev.
set -xe
docker-compose down
mkdir workdir || true
chmod a+rwX workdir
cargo clippy
cargo clippy --features discord
docker buildx build . -t spritecollab-srv
docker-compose up --no-log-prefix
