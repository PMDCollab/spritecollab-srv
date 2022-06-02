FROM ekidd/rust-musl-builder:stable as builder

RUN USER=root cargo new --bin spritecollab-srv
WORKDIR ./spritecollab-srv
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml
RUN cargo build --release --features discord  # collects dependencies
RUN rm src/*.rs  # removes the `cargo new` generated files.

ADD . ./

RUN rm ./target/*/release/deps/spritecollab_srv*

RUN cargo build --release --features discord

FROM alpine:latest

ARG APP=/usr/src/app

EXPOSE 3000

ENV TZ=Etc/UTC \
    APP_USER=spritecollab

RUN addgroup -S $APP_USER \
    && adduser -S -g $APP_USER $APP_USER

RUN apk update \
    && apk add --no-cache ca-certificates tzdata \
    && rm -rf /var/cache/apk/*

COPY --from=builder /home/rust/src/spritecollab-srv/target/x86_64-unknown-linux-musl/release/spritecollab-srv ${APP}/spritecollab-srv

RUN chown -R $APP_USER:$APP_USER ${APP}

USER $APP_USER
WORKDIR ${APP}

STOPSIGNAL SIGINT

ENTRYPOINT ["./spritecollab-srv"]
