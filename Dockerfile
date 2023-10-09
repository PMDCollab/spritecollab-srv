FROM rust:slim-buster as builder

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
RUN cargo build --release --features discord  # collects dependencies
RUN rm src/*.rs  # removes the `cargo new` generated files.

ADD . ./

RUN rm ./target/release/deps/spritecollab_srv*

RUN cargo build --release --features discord
RUN strip /src/spritecollab-srv/target/release/spritecollab-srv


FROM rust:slim-buster as build

ARG APP=/usr/src/app

EXPOSE 34434

ENV TZ=Etc/UTC \
    APP_USER=spritecollab

RUN adduser --system --group $APP_USER

RUN apt-get update && apt-get install -y \
  ca-certificates \
  tzdata \
  && rm -rf /var/lib/apt/lists/*


COPY --from=builder /src/spritecollab-srv/target/release/spritecollab-srv ${APP}/spritecollab-srv

RUN chown -R $APP_USER:$APP_USER ${APP}

USER $APP_USER
WORKDIR ${APP}

ENTRYPOINT ["./spritecollab-srv"]
