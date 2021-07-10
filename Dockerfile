# Build Stage
FROM rust:latest AS builder
WORKDIR /usr/src/
RUN rustup target add x86_64-unknown-linux-musl

RUN git clone https://github.com/Kiiyya/rcon_cli.git ./rcon_cli

WORKDIR /usr/src/rcon_cli

RUN cargo install --target x86_64-unknown-linux-musl --path .

# Bundle Stage
FROM scratch

COPY --from=builder /usr/local/cargo/bin/rcon_cli .
USER 1000
CMD ["./rcon_cli"]
