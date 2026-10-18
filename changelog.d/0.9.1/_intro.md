### Added

- **Backends deployed elsewhere.** An MCP entry may give `url` instead of
  `image`, and the gateway routes to it without deploying anything. For
  backends whose pod needs what a generic backend deployment cannot give it —
  a particular node, host volumes, a host port — such as a Soulseek client
  sharing a music library. Whoever deploys that pod owns its Service and its
  NetworkPolicy; the gateway's backend policy selects only the pods it
  deploys.
