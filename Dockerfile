# syntax=docker/dockerfile:1.7
FROM rust:1.97.0-bookworm@sha256:8fa55b2f3ddf97471ab6a767bfa3f37e6bad0986ba823e75fea57e2a2a5c3073 AS builder
WORKDIR /source
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=secret,id=github_token,required=true \
    --mount=type=cache,id=workspace-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=workspace-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=workspace-target,target=/source/target,sharing=locked \
    token="$(cat /run/secrets/github_token)" && \
    git config --global url."https://x-access-token:${token}@github.com/beyond10x/".insteadOf "https://github.com/beyond10x/" && \
    cargo build --locked --release -p workspace-service && \
    git config --global --unset-all url."https://x-access-token:${token}@github.com/beyond10x/".insteadOf && \
    install -D /source/target/release/workspace-service /out/workspace-service

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:adcd20c7b4c988b73cbfbddb26d2eee574571e6d7c9ffea29b3821e0690efb77
ARG SOURCE_SHA=unknown
LABEL org.opencontainers.image.revision=$SOURCE_SHA
LABEL org.opencontainers.image.source=https://github.com/beyond10x/workspace
COPY --from=builder /out/workspace-service /usr/local/bin/workspace-service
EXPOSE 8094
ENTRYPOINT ["/usr/local/bin/workspace-service"]
