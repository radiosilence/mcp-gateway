import { describe, expect, it } from "vitest";
import * as yaml from "yaml";
import { McpSchema } from "./mcp-gateway.schemas.ts";
import { hydraIngress, mcpRegistry } from "./mcp-gateway.ts";

/**
 * The registry is the contract with the gateway: it is parsed at boot, and a
 * key spelled our way rather than its way is a crash loop, not a warning. So
 * these assert the wire format, not our config's.
 */
const svcDns = (name: string) => `${name}.ns.svc.cluster.local`;

describe("mcpRegistry", () => {
  const parse = (mcps: unknown[]) =>
    yaml.parse(
      mcpRegistry(
        mcps.map((m) => McpSchema.parse(m)),
        svcDns,
      ),
    );

  it("derives backend URLs from the Services declared alongside", () => {
    const [entry] = parse([
      {
        id: "caldav",
        name: "Calendar",
        image: "x",
        graphqlPath: "/graphql",
        credentialHeader: "X-Token",
      },
    ]);
    expect(entry.backend).toBe(
      "http://mcp-gateway-mcp-caldav.ns.svc.cluster.local:8080/mcp",
    );
    expect(entry.graphql).toBe(
      "http://mcp-gateway-mcp-caldav.ns.svc.cluster.local:8080/graphql",
    );
  });

  it("uses a URL backend as given, for its MCP and GraphQL paths", () => {
    const [entry] = parse([
      {
        id: "slsk",
        name: "Soulseek",
        url: "http://slsk-internal:8081/",
        graphqlPath: "/graphql",
        credentialHeader: "X-Token",
      },
    ]);
    expect(entry.backend).toBe("http://slsk-internal:8081/mcp");
    expect(entry.graphql).toBe("http://slsk-internal:8081/graphql");
  });

  it("renames camelCase config onto the gateway's snake_case keys", () => {
    const [entry] = parse([
      {
        id: "caldav",
        name: "Calendar",
        image: "x",
        keyHelpUrl: "https://example.test",
        fields: [
          {
            id: "calendar",
            label: "Calendar",
            header: "X-CalDAV-Calendar",
            required: false,
            optionsQuery: "{ options { value label } }",
            syncMutation: "mutation($value: String!) { set(id: $value) }",
          },
        ],
      },
    ]);
    expect(entry.key_help_url).toBe("https://example.test");
    expect(entry.fields[0]).toMatchObject({
      options_query: "{ options { value label } }",
      sync_mutation: "mutation($value: String!) { set(id: $value) }",
      required: false,
    });
  });

  it("omits what was not configured rather than emitting empty keys", () => {
    const [entry] = parse([
      { id: "folk", name: "Folk", image: "x", public: true },
    ]);
    expect(entry).toEqual({
      id: "folk",
      name: "Folk",
      backend: "http://mcp-gateway-mcp-folk.ns.svc.cluster.local:8080/mcp",
      public: true,
    });
  });
});

describe("hydraIngress", () => {
  const gateway = { podSelector: { matchLabels: { app: "mcp-gateway" } } };
  const traefik = { "app.kubernetes.io/name": "traefik" };

  it("admits only the gateway and named callers to each port", () => {
    expect(
      hydraIngress({ admin: [{ app: "auth" }], public: [traefik] }),
    ).toEqual([
      {
        from: [gateway, { podSelector: { matchLabels: { app: "auth" } } }],
        ports: [{ protocol: "TCP", port: 4445 }],
      },
      {
        from: [gateway, { podSelector: { matchLabels: traefik } }],
        ports: [{ protocol: "TCP", port: 4444 }],
      },
    ]);
  });

  // An empty `from` admits every source, which would open the port it was
  // meant to close.
  it("never emits a rule with an empty `from`", () => {
    for (const rule of hydraIngress({ admin: [], public: [] }))
      expect(rule.from).toEqual([gateway]);
  });

  // `matchLabels: {}` selects every pod in the namespace.
  it("refuses an empty label set on either port", () => {
    expect(() => hydraIngress({ admin: [{}], public: [] })).toThrow(
      /empty label set/,
    );
    expect(() => hydraIngress({ admin: [], public: [traefik, {}] })).toThrow(
      /empty label set/,
    );
  });
});
