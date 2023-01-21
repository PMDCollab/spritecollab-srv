#!/usr/bin/env sh
# Full clippy-checked Docker build for local dev.
set -xe
docker-compose down
mkdir -p workdir
chmod a+rwX workdir
mkdir -p db-data
mkdir -p mq-data

cargo clippy --package spritecollab-srv -- -D warnings
cargo clippy --package spritecollab-srv --features discord -- -D warnings
cargo clippy --package spritecollab-srv --features activity -- -D warnings
cargo clippy --package spritecollab-srv --features discord,activity -- -D warnings
#docker buildx build . -t ghcr.io/pmdcollab/spritecollab-srv:no-discord --build-arg features=""
#docker buildx build . -t ghcr.io/pmdcollab/spritecollab-srv:latest
docker buildx build . -t ghcr.io/pmdcollab/spritecollab-srv:activity --build-arg features="activity"

cargo clippy --package spritecollab-pub -- -D warnings
docker buildx build . -f spritecollab-pub/Dockerfile -t ghcr.io/pmdcollab/spritecollab-srv:spritecollab-pub-latest

docker-compose up
