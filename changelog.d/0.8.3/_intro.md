### Fixed

- **The dashboard waited on every backend before it rendered anything.** A page
  load spent 600ms on a settings prefetch and then ran one credential check per
  configured MCP, sequentially, each bounded only by the HTTP client's 15s
  timeout. Against the live deployment that was 1.0–1.6s to first byte where
  `/healthz` answered in 75ms, and two unresponsive backends could have held the
  page open for half a minute.

  Nothing that has to ask somebody else runs during a render now. `McpView::build`
  reads this gateway's own Postgres and returns; a setting's choices load over
  htmx as they already did, and the connection badge does the same through a new
  `/dashboard/{mcp}/status`. Only an MCP with a `verify` block and a stored
  credential shows a pending pill — every other badge is a fact about our own
  database and still arrives settled in the first response.

  The prefetch is deleted rather than retuned. Its own logs said `missed=1 of=1`
  on every request: it had never once beaten its budget, so what it bought was
  600ms and then the same htmx fetch afterwards.

  The failure mode worth knowing about is that the answer renders from the same
  template as the question — a settled badge that kept its URL would carry
  another `hx-trigger="load"` and refetch itself for as long as the tab stayed
  open. That is what clearing `status_url` in `checked()` prevents, and there is
  a test on it.
