# syntax=docker/dockerfile:1
# Alpine is musl-native on both arm64 and amd64, so the binary is static without cross-compiling.
FROM rust:1-alpine AS chef
RUN apk add --no-cache musl-dev build-base cmake perl
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Dependencies first: this layer is cached until Cargo.toml changes.
RUN cargo chef cook --release --recipe-path recipe.json
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --release \
 && mkdir /out \
 && cp target/release/jamhead /out/jamhead \
 && ln -s jamhead /out/jh

FROM scratch
COPY --from=builder /out/ /usr/local/bin/
ENV PATH=/usr/local/bin
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["jamhead"]
CMD ["serve", "--data", "/data", "--bind", "0.0.0.0:8080"]
