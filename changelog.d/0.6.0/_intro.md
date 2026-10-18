Released 2026-08-07.

Every dependency moved to its current release. Most of that is invisible, but
two changes below alter runtime behaviour — read those before deploying.

### Changed

- **`reqwest` 0.12 → 0.13 changes where TLS trust comes from.** 0.13 drops the
  compiled-in Mozilla root store entirely; its `rustls` feature now pulls
  `rustls-platform-verifier`, which reads the *system* trust store. The
  published image already ships a CA bundle for exactly this reason, so it is
  unaffected — but the bare binary now needs `/etc/ssl/certs/ca-certificates.crt`
  (or the platform equivalent) on any host that runs it, and will fail to build
  a client without one. `rustls-tls` was also renamed to `rustls`.
- **`sqlx` 0.8 → 0.9, moved from `tls-rustls-ring` to `tls-rustls-aws-lc-rs`.**
  reqwest's `rustls` feature hard-selects the aws-lc-rs provider, and two
  rustls crypto providers in one binary is how you get a process with no
  unambiguous default. Matching sqlx to it keeps exactly one.
- **`askama` 0.12 → 0.16 makes `{% call %}` a block**, so every macro call now
  carries a matching `{% endcall %}`. Rendered output was diffed against 0.12
  across every template branch and is byte-for-byte identical.
- **`rand` 0.8 → 0.10.** The `RngCore` trait is gone (the old `Rng` extension
  trait took its name); token generation uses `rand::fill` instead.
- **`chacha20poly1305` 0.10 → 0.11** (aead 0.6): nonces come from the `Generate`
  trait, and `Array::from_slice` is deprecated in favour of `TryFrom`. Two
  panics became errors as a result — a `TOKEN_ENC_KEY` of the wrong length and a
  stored blob with a malformed nonce are now reported, not a crash.
- `axum` 0.8.9, `tower-http` 0.6 → 0.7, `base64` 0.22 → 0.23, and the rest of
  the tree to current.
