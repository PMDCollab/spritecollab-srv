# Build this image with .. as context!
FROM rust:slim-buster as builder

RUN apt-get update && apt-get install -y \
  libssl-dev \
  pkg-config \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /src
RUN USER=root cargo new --bin spritecollab-pub
WORKDIR /src/spritecollab-pub
COPY ./Cargo.lock ./Cargo.lock
COPY ./spritecollab-pub/Cargo.toml ./Cargo.toml
COPY ./sc-common-db/Cargo.toml ../sc-common-db/Cargo.toml
RUN mkdir -p ../sc-common-db/src && touch ../sc-common-db/src/lib.rs
RUN cargo build --release  # collects dependencies
RUN rm src/*.rs  # removes the `cargo new` generated files.

ADD sc-common-db ../sc-common-db/
ADD spritecollab-pub ./

RUN rm ./target/release/deps/spritecollab_pub* && (rm ./target/release/deps/sc_common_db* || echo "WARNING: sc-common-db was not generated.")

RUN cargo build --release
RUN strip /src/spritecollab-pub/target/release/spritecollab-pub


FROM rust:slim-buster as build

ARG APP=/usr/src/app

EXPOSE 3001

ENV TZ=Etc/UTC \
    APP_USER=depositbox \
    RUST_LOG="spritecollab_pub=info"

RUN adduser --system --group $APP_USER

RUN apt-get update && apt-get install -y \
  ca-certificates \
  tzdata \
  && rm -rf /var/lib/apt/lists/*


COPY --from=builder /src/spritecollab-pub/target/release/spritecollab-pub ${APP}/spritecollab-pub

RUN chown -R $APP_USER:$APP_USER ${APP}

USER $APP_USER
WORKDIR ${APP}

ENTRYPOINT ["./spritecollab-pub"]
