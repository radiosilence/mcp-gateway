import { defineConfig } from "rolldown";

export default defineConfig({
  input: "web/app.ts",
  output: { file: "assets/app.js", format: "esm", minify: true },
});
