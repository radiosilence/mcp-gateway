/**
 * The version this chart deploys, and the images beside it.
 *
 * `APP_VERSION` is this repository's own crate version, and the package version
 * is the same number. That is deliberate: the gateway and the chart that
 * deploys it are one project and always ship together, so one number is honest
 * where two would only invite them to disagree. Pinning
 * `@radiosilence/mcp-gateway-pulumi@0.7.1` therefore says exactly which gateway
 * you get. CI refuses a release where `Cargo.toml`, `package.json` and this
 * disagree.
 */
export const APP_VERSION = "0.8.1";

export const VERSIONS = {
  gateway: `ghcr.io/radiosilence/mcp-gateway:v${APP_VERSION}`,
} as const;

/**
 * The two images the gateway needs beside it, both moved by hand.
 *
 * Stated as their own const rather than merely left out of anything: which pins
 * are watched and which are a judgement should be readable here, not inferred
 * from an absence.
 */
export const UNTRACKED = {
  /**
   * Postgres publishes patches into the major tag, which is the whole of what
   * is wanted from it. There is no release to follow.
   */
  postgres: "postgres:16",
  /**
   * Moved with the release notes open. Hydra owns the database holding every
   * registered client, and a major bump needs `hydra migrate sql` run against
   * it — Ory's unified versioning puts "latest" more than twenty majors ahead
   * of the schema this database was created by, so an unattended bump is every
   * client of this gateway unable to log in at once.
   */
  hydra: "oryd/hydra:v2.2.0",
} as const;
