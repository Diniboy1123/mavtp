FROM rust:alpine AS builder

RUN apk add --no-cache \
    musl-dev \
    musl-utils \
    build-base \
    openssl-dev \
    openssl-libs-static

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release

# we cannot use scratch, because we need a ca cert bundle
FROM alpine:latest

COPY --from=builder /app/target/release/mavtp /mavtp

EXPOSE 123/udp

CMD ["/mavtp"]
