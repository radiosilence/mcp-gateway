### Changed

- **htmx replaced by Datastar, which is what mariastew uses.** One front-end
  model across the estate rather than two, and one set of traps learned once.

  Every fragment endpoint now answers with a `datastar-patch-elements` SSE
  event rather than bare HTML — the same `elements`-prefixed payload mariastew
  sends, so the two repositories mean the same thing by a patch. The client
  morphs by element id, which is the real shape of the change: htmx targeted by
  relation (`closest section`, `next [data-status]`) so markup only had to be
  *positioned* right, where now it has to be *named* right. Every patchable
  thing carries an id.

  `data-init` is the lazy-load trigger, not `data-on-load` — Datastar has no
  `load` plugin at all. Getting that wrong is not an error: the attribute
  matches nothing and the control renders, looks correct and does nothing.
  `every_datastar_attribute_is_one_datastar_matches` reads the plugin names out
  of the vendored bundle and refuses any attribute that names none of them.

  A form keeps its native validation: `data-on:submit` on a `<form>` prevents
  the default itself and runs `checkValidity` before posting, so `required`
  still means something.

### Security

- **`script-src` now allows `unsafe-eval`.** Datastar compiles each `data-*`
  expression with `Function()`, so without it no attribute on the page does
  anything; serving the bundle ourselves does not change that, because eval is
  about turning strings into code rather than about where the file came from.

  `script-src 'self'` still refuses script from anywhere else, so nothing new
  can be *loaded*. What it costs is that a `data-*` attribute built from an
  unescaped value would be executable and the policy would no longer be what
  stopped it — and this page renders backend-supplied strings, including the
  error text of a failed lookup. Askama escapes by default and nothing here
  opts out; `no_template_opts_out_of_escaping` is the control that had this as
  a backstop and now stands on its own.

  The policy is otherwise unchanged, and is now shared verbatim with mariastew.
