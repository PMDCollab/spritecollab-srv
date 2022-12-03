FROM rust:slim-buster as builder

ARG features="discord,discord-reputation"

RUN apt-get update && apt-get install -y \
  libssl-dev \
  pkg-config \
  libglib2.0-dev \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /src
RUN USER=root cargo new --bin spritecollab-srv
WORKDIR /src/spritecollab-srv
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml
COPY ./sc-common-db/Cargo.toml ./sc-common-db/Cargo.toml
COPY ./spritecollab-pub/Cargo.toml ./spritecollab-pub/Cargo.toml
RUN mkdir -p ./spritecollab-pub/src && touch ./spritecollab-pub/src/main.rs && \
    mkdir -p ./sc-common-db/src && touch ./sc-common-db/src/lib.rs
RUN cargo build --bin spritecollab-srv --features "${features}" --release  # collects dependencies
RUN rm src/*.rs  # removes the `cargo new` generated files.

ADD . ./

RUN rm ./target/release/deps/spritecollab_srv* && (rm ./target/release/deps/sc_common_db* || echo "WARNING: sc-common-db was not generated.")

RUN cargo build --bin spritecollab-srv --features "${features}" --release
RUN strip /src/spritecollab-srv/target/release/spritecollab-srv


FROM rust:slim-buster as build

ARG APP=/usr/src/app

EXPOSE 34434

ENV TZ=Etc/UTC \
    APP_USER=depositbox \
    RUST_LOG="spritecollab_srv=info"

RUN adduser --system --group $APP_USER

RUN apt-get update && apt-get install -y \
  ca-certificates \
  tzdata \
  && rm -rf /var/lib/apt/lists/*


COPY --from=builder /src/spritecollab-srv/target/release/spritecollab-srv ${APP}/spritecollab-srv

RUN chown -R $APP_USER:$APP_USER ${APP}

USER $APP_USER
WORKDIR ${APP}

STOPSIGNAL SIGINT

ENTRYPOINT ["./spritecollab-srv"]
