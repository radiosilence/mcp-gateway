Released 2026-07-26.

### Fixed

- **Half the options timing was never actually added.** 0.3.1 said both the
  prefetch and the endpoint that serves what it misses were timed. Only the
  prefetch ever was — the edit adding the second one silently matched nothing,
  and compiling and passing tests does not notice an edit that did nothing. So
  the half that measures a slow backend, which was the reason for any of it, was
  missing. It is there now.

### Changed

- **One fetch behind both paths.** Asking a backend for a setting's choices —
  credentials, the query, reading the answer, working out which is stored — was
  written once for the page's prefetch and again for the endpoint htmx calls.
  They render the same control, so asking different questions was only ever a
  matter of time.

- **One definition of the status line.** The page rendered an empty one and a
  save replaced it, from two pieces of markup that had already drifted once.
