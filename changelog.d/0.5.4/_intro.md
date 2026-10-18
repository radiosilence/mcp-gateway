Released 2026-07-26.

### Changed

- **LTO and codegen-units tuned for gateway workload.** Fat LTO with a single
  codegen unit was the slowest possible build configuration, serializing
  whole-program optimization across the whole dependency tree. The gateway is
  I/O-bound — it proxies HTTP and talks to Postgres — and gains nothing
  measurable from that cost. Thin LTO with 16 codegen units instead.
