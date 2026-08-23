# `@radiosilence/mcp-gateway-pulumi`

Stands this repository's gateway up in Kubernetes: the gateway itself, the Ory
Hydra it delegates OAuth to, the Postgres holding both their state, and one
Deployment and Service per backend MCP.

```ts
import { createMcpGateway } from "@radiosilence/mcp-gateway-pulumi";

const { routes, oidc } = createMcpGateway(
  provider,
  namespace,
  {
    replicas: 2,
    limits: { cpu: "250m", memory: "256Mi" },
    mcps: [
      { id: "tfl", name: "TfL", image: "ghcr.io/…", port: 8080, path: "/mcp" },
    ],
  },
  {
    hostname: "mcp.example.com",
    authHostname: "auth.example.com",
    oidcClientId: "mcp-gateway",
    oidcClientSecret: someSecret,
  },
);
```

## What it does not do

It has no address, no credentials and no opinion about what it fronts. Hostnames,
secrets and the list of MCPs belong to the deployment that instantiates this, and
are passed in — the same split `DEPLOY.md` describes for the compose file.

It also states no scheduling policy. `limits` is a ceiling you choose; `requests`
is left to you, because how much of a ceiling to reserve depends on what else
shares the node. Omit it and Kubernetes defaults the request to the limit.

What comes back is where the gateway now stands and the OAuth client it needs
registered — including its redirect URI, built from the hostname it was told to
publish at, so the allowlist entry cannot name somewhere the service is not.

## Peer dependencies

`@pulumi/pulumi`, `@pulumi/kubernetes`, `@pulumi/random` and `zod` are peers.
A Pulumi program must have exactly one copy of each; two would be two engines,
two provider registries, and two mutually unassignable sets of Zod types.

## Versioning

The package version is the crate version. They are one project and always ship
together, so one number is honest where two would only invite them to disagree
— pinning `@radiosilence/mcp-gateway-pulumi@0.8.0` says exactly which gateway
you get, and CI refuses a release where `Cargo.toml`, `package.json` and
`src/versions.ts` disagree.

## Installing

Published to GitHub Packages, which requires authentication even for public
packages. Consumers need a `read:packages` token:

```
# .npmrc
@radiosilence:registry=https://npm.pkg.github.com
//npm.pkg.github.com/:_authToken=${NODE_AUTH_TOKEN}
```

In Actions, `secrets.GITHUB_TOKEN` is enough.
