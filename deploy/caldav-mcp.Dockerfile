# Backend MCP: caldav-cli in HTTP mode. Tracks main until the first release
# lands, then pin a tag — a feature branch would break the build the moment it
# is merged and deleted. Unlike the fastmail image there is no native toolchain
# to install: caldav-cli has no kreuzberg/pdfium dependency, so this is a
# plain, fast Rust build.

FROM rust:1-bookworm AS build
RUN cargo install --locked \
    --git https://github.com/radiosilence/caldav-cli \
    --branch main \
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
