### Fixed

- **Each MCP is its own protected resource.** The 401 challenge for `/{id}`
  names `/.well-known/oauth-protected-resource/{id}`, whose `resource` is
  `{public_url}/{id}`. Clients that check the metadata's `resource` against
  the URL they connected to (RFC 9728 §3.3) rejected the gateway-wide
  document and never reached the authorization server. The gateway-wide
  document is still served for clients that look there.
