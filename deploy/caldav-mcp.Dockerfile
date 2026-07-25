# Backend MCP: caldav-cli in HTTP mode. Pinned to a release tag: `--branch main`
# sits inside a RUN layer that Docker caches, so a rebuild would happily reuse a
# months-old binary with nothing to show for it. Bumping the tag is a visible
# one-line diff. Unlike the fastmail image there is no native toolchain to
# install: caldav-cli has no kreuzberg/pdfium dependency, so this is a plain,
# fast Rust build.

FROM rust:1-bookworm AS build
RUN cargo install --locked \
    --git https://github.com/radiosilence/caldav-cli \
    --tag v0.3.0 \
    caldav-cli

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/cargo/bin/caldav-cli /usr/local/bin/caldav-cli
EXPOSE 8080
RUN useradd --system --uid 10001 --create-home app
USER app
ENTRYPOINT ["caldav-cli"]
CMD ["mcp", "--http", "0.0.0.0:8080"]
