# The gateway is a lean axum app — it links no MCP, so no pdfium/kreuzberg
# toolchain and a fast build. Templates (askama) and migrations (sqlx::migrate!)
# are embedded; mcps.json is mounted at runtime.

FROM rust:1-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/mcp-gateway /usr/local/bin/mcp-gateway
EXPOSE 8080
ENV BIND_ADDR=0.0.0.0:8080
ENTRYPOINT ["/usr/local/bin/mcp-gateway"]
