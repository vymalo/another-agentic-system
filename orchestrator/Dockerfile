# syntax=docker/dockerfile:1.7
#
# The orchestrator image. Build context: this directory (`orchestrator/`).
#
#   docker build -t orchestrator orchestrator
#
# Dependencies are compiled in their own layer (cargo-chef), so a change to the sources
# does not rebuild them. Builder and runtime are both Debian 12 (glibc 2.36): the dynamically
# linked binary runs unchanged on the distroless `cc` image, which also carries the CA
# certificates the A2A client needs for https agent cards. The Rust version is pinned by the
# builder tag (`rust-toolchain.toml` is left out of the context, see `.dockerignore`).
#
# Cargo features of the binary are a build argument. The default is none of the optional ones,
# so the image links no in-process agents (ADR 0015); `--build-arg ORCH_FEATURES=agent-local`
# builds one that can host them. Its value is the same for the dependency layer and the build.

FROM lukemathwalker/cargo-chef:0.1.77-rust-1.94.1-bookworm AS chef
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
ARG ORCH_FEATURES=""
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json --package orchestrator \
    --features "$ORCH_FEATURES"
COPY . .
RUN cargo build --release --locked --package orchestrator --features "$ORCH_FEATURES"
# The directory of the directory artifact store (ADR 0032), made here because the final image has no shell: copied
# with its owner, so a volume mounted on it is writable by the runtime user the first time it is used.
RUN mkdir -p /out/var/lib/orchestrator/artifacts

FROM gcr.io/distroless/cc-debian12:nonroot
LABEL org.opencontainers.image.source="https://github.com/vymalo/another-agentic-system" \
      org.opencontainers.image.description="Orchestration layer: chat API and durable A2A delegation" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
COPY --from=build /src/target/release/orchestrator /orchestrator
COPY --from=build --chown=65532:65532 /out/var/lib/orchestrator /var/lib/orchestrator
# Numeric, so Kubernetes can verify `runAsNonRoot` (this is distroless's `nonroot` user).
USER 65532:65532
ENV LISTEN_ADDR=0.0.0.0:8080 \
    LOG_FORMAT=json
EXPOSE 8080
ENTRYPOINT ["/orchestrator"]
