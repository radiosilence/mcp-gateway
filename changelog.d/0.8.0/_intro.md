### Added

- **A Pulumi component, published from this repo as
  `@radiosilence/mcp-gateway-pulumi`.** The topology `docker-compose.yml`
  describes, translated for Kubernetes: the gateway, Hydra, Postgres, and one
  Deployment and Service per backend MCP. It carries no hostname, no credential
  and no list of MCPs — those still belong to the deployment that instantiates
  it, which is the same split `DEPLOY.md` already described for compose.

  It lived in the deployment until now, which made every consumer of this
  gateway reimplement how to stand it up, and meant a change to the gateway and
  a change to how it is deployed landed in different repositories with nothing
  tying them together. The package version is the crate version for that reason:
  pinning `@radiosilence/mcp-gateway-pulumi@0.8.0` says exactly which gateway
  you get, and CI refuses a release where the two disagree.

  The chart publishes only after the image it pins is in the registry, for the
  same reason the GitHub release is cut last.
