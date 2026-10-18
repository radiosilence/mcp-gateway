Released 2026-07-26.

### Fixed

- **A settings dropdown flashed a raw identifier before it resolved.** An
  option is keyed by its id rather than its label, because labels are not
  guaranteed unique, so a stored value showed as a bare identifier for as long
  as the choices took to arrive.

  It is a skeleton now: a `<select>` carrying the control's own classes, pulsing
  with no text and no arrow, replaced outright once the choices land. The same
  element and the same classes as the thing that replaces it, so the two cannot
  differ in height and nothing reflows. Rendering it as the initial state rather
  than styling a request-in-flight class also means it is a skeleton from first
  paint rather than from whenever the request starts, which is the gap it exists
  to cover.
