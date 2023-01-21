# Build this image with .. as context!
FROM rust:slim-buster as builder

RUN apt-get update && apt-get install -y \
  libssl-dev \
  pkg-config \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /src
RUN USER=root cargo new --bin spritecollab-pub
WORKDIR /src/spritecollab-pub
COPY ./Cargo.lock ../Cargo.lock
COPY ./Cargo.toml ../Cargo.toml
COPY ./spritecollab-pub/Cargo.toml ./Cargo.toml
COPY ./sc-common/Cargo.toml ../sc-common/Cargo.toml
# We are not going to use this crate at all, but we need to have a dummy of it for the
# workspace.
COPY ./sc-activity-rec/Cargo.toml ../sc-activity-rec/Cargo.toml
RUN mkdir -p ../src && \
    touch ../src/lib.rs && \
    mkdir -p ../sc-activity-rec/src && \
    touch ../sc-activity-rec/src/lib.rs && \
    mkdir -p ../sc-common/src && \
    touch ../sc-common/src/lib.rs
RUN cargo build --release  # collects dependencies
RUN rm src/*.rs  # removes the `cargo new` generated files.

ADD sc-common ../sc-common/
ADD spritecollab-pub ./

RUN rm ../target/release/deps/spritecollab_pub* && rm ../target/release/deps/libsc_common*

RUN cargo build --release
RUN strip /src/target/release/spritecollab-pub


FROM rust:slim-buster as build

ARG APP=/usr/src/app

EXPOSE 3001

ENV TZ=Etc/UTC \
    APP_USER=spritecollab_pub \
    RUST_LOG="spritecollab_pub=info"

RUN adduser --system --group $APP_USER

RUN apt-get update && apt-get install -y \
  ca-certificates \
  tzdata \
  && rm -rf /var/lib/apt/lists/*


COPY --from=builder /src/target/release/spritecollab-pub ${APP}/spritecollab-pub

RUN chown -R $APP_USER:$APP_USER ${APP}

USER $APP_USER
WORKDIR ${APP}

STOPSIGNAL SIGINT

ENTRYPOINT ["./spritecollab-pub"]
