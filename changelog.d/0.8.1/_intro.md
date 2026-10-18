### Fixed

- **The published package contained no code.** `0.8.0`'s tarball held
  `package.json` and the README and nothing else: the chart is built in the
  `Chart` job and published from another, on another runner, and nothing
  carried `dist/` between them. `prepublishOnly` now builds as part of
  `npm publish`, so the tarball cannot come from a tree that was never
  compiled — and CI checks what `npm pack` would actually ship rather than
  what the build left behind, which is the check that would have caught it.
