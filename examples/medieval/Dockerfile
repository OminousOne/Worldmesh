# syntax=docker/dockerfile:1
# One image for every server role of the medieval example; the compose file picks the role per
# service. Built from the repository root (it needs the whole workspace):
#   docker build -f examples/medieval/Dockerfile .

FROM rust:1-trixie AS build
WORKDIR /src
COPY . .
# Cache mounts keep crate downloads and build output between builds.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release -p medieval \
    && cp target/release/medieval /usr/local/bin/medieval

FROM debian:trixie-slim
COPY --from=build /usr/local/bin/medieval /usr/local/bin/medieval
ENTRYPOINT ["medieval"]
