# Backend MCP: fastmail-cli in HTTP mode. Built from the branch until the
# per-request-token change lands in a release (then pin a tag). This is the
# heavy build (kreuzberg + bundled pdfium) — but it's a separate image, built
# once, independent of the fast-iterating gateway.

FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*
# Build from the rebinding-fix branch until it merges (#35), then switch to a
# release tag (see the "release fastmail-cli + pin" step).
RUN cargo install --locked \
    --git https://github.com/radiosilence/fastmail-cli \
    --branch fix/mcp-http-rebinding \
    fastmail-cli

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/cargo/bin/fastmail-cli /usr/local/bin/fastmail-cli
EXPOSE 8080
ENTRYPOINT ["fastmail-cli"]
CMD ["mcp", "--http", "0.0.0.0:8080"]
