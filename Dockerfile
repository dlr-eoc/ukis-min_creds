FROM rust:1-slim-bullseye as builder

RUN apt-get update && \
   apt-get install --yes --no-install-recommends libssl-dev build-essential pkg-config

COPY min_creds /tmp/min_creds

RUN cd /tmp/min_creds && \
    cargo build --release && \
    strip target/release/min_creds

FROM debian:bullseye-slim

COPY min_creds/example.config.yaml /
COPY --from=builder /tmp/min_creds/target/release/min_creds /usr/local/bin
ENTRYPOINT ["/usr/local/bin/min_creds"]
