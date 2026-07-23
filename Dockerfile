# Templates (askama) and migrations (sqlx::migrate!) are embedded into the
# binary at compile time, so the runtime image needs neither directory — just
# the binary and CA certs.

FROM rust:1-bookworm AS build
WORKDIR /app
# Build deps for the fastmail-cli dep tree (kreuzberg / bundled-pdfium, etc.).
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/fastmail-mcp-service /usr/local/bin/fastmail-mcp-service
EXPOSE 8080
ENV BIND_ADDR=0.0.0.0:8080
ENTRYPOINT ["/usr/local/bin/fastmail-mcp-service"]
