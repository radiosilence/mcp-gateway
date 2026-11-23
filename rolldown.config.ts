import { defineConfig } from "rolldown";

export default defineConfig({
  input: "web/app.ts",
  // askama_datastar serves Datastar; main.rs points the import at its address.
  external: ["datastar"],
  output: { file: "assets/app.js", format: "esm", minify: true },
});
