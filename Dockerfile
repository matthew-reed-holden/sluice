# Stage 1: Build
FROM rust:1.82-slim-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler pkg-config && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

RUN cargo build --release -p sluice-server

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/sluice /usr/local/bin/sluice

RUN useradd --create-home --shell /bin/bash sluice
USER sluice

VOLUME /var/lib/sluice

EXPOSE 50051 9090

ENV SLUICE_HOST=0.0.0.0
ENV SLUICE_PORT=50051
ENV SLUICE_DATA_DIR=/var/lib/sluice

ENTRYPOINT ["sluice"]
