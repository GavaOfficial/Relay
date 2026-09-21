# Costruire dalla root del repo:  docker build -f api/server/Dockerfile -t relay-server .
FROM rust:1 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
COPY common ./common
COPY api/server ./api/server
RUN cargo build --release -p relay-server

FROM debian:bookworm-slim
# ffmpeg servira' in fase 8 per creare l'MP4 finale
RUN useradd -r -u 10001 relay && mkdir /data && chown relay /data
COPY --from=build /src/target/release/relay-server /usr/local/bin/relay-server
USER relay
ENV RELAY_BIND=0.0.0.0:8080 RELAY_DATA=/data
VOLUME /data
EXPOSE 8080
CMD ["relay-server"]
