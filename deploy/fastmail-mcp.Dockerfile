# Backend MCP: fastmail-cli in HTTP mode, pinned to a release tag — see the
# caldav image for why a branch pin goes stale silently. This is the heavy build
# (kreuzberg + bundled pdfium), but it's a separate image, built once,
# independent of the fast-iterating gateway.

FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install --locked \
    --git https://github.com/radiosilence/fastmail-cli \
    --tag v3.1.2 \
    fastmail-cli

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/cargo/bin/fastmail-cli /usr/local/bin/fastmail-cli
EXPOSE 8080
RUN useradd --system --uid 10001 --create-home app
USER app
ENTRYPOINT ["fastmail-cli"]
CMD ["mcp", "--http", "0.0.0.0:8080"]
