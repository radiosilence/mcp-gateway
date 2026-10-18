Released 2026-07-26.

### Changed

- **Saving and disconnecting no longer reload the page.** Both post over htmx
  and are answered with the section they changed, so the badge, the timestamp
  and the settings all update together — and a wrong password corrected in the
  form immediately shows the choices the new one can see, which previously took
  a round trip through a redirect.

  The section became a template of its own for this, rendered by the page and
  again on its own, so the two cannot describe the same MCP differently. What a
  mutation wants to say is carried on the section rather than in a flash
  parameter, which no longer has a page load to survive.

- **Disconnect confirms via `hx-confirm`.** The handler that did it listened for
  form submission, and htmx binds that on the element — it would have fired the
  request before the confirmation ran. Worth stating plainly because it would
  have looked fine and quietly stopped asking.

### Internal

- Being signed in is an extractor rather than four hand-written guards. The
  failure mode for a fifth handler that forgot one is an unauthenticated
  endpoint on a credential vault, which is not a thing to leave to memory. Two
  of them, because a browser wants the login page and an htmx fragment wants an
  error — a redirect there would be followed and swapped into the page.
- `dashboard.rs` was 890 lines doing three jobs; it is now handlers, the view
  and its constructor, and the options fetching, with each module's tests beside
  the code they cover.
