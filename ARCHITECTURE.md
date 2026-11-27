# Architecture

mcp-gateway is one binary: an OAuth resource server that proxies each
registered MCP with the caller's stored credentials injected, and a small web
UI where a person signs in and manages those credentials. The README explains
why the gateway exists and how it is deployed; this document describes how
the code is put together, and the web UI in detail, since it follows the
layout radiosilence's server-rendered apps share
([slsk-mcp's ARCHITECTURE.md](https://github.com/radiosilence/slsk-mcp/blob/main/ARCHITECTURE.md)
is the reference).

## The service

`main.rs` reads the configuration (`Config::from_env`), opens the store, and
serves one router: the web UI (`web::routes`), `/healthz`, the OAuth
protected-resource metadata (`well_known`) and, at `/{id}`, the proxy to each
MCP (`proxy`). Configuration rejects an MCP id that would shadow a route the
gateway serves itself.

- **Auth** for MCP clients is a bearer token Hydra issued, checked by
  introspection (`auth::hydra`). The web UI's session is a cookie naming a row
  in the store, established by an OIDC round trip through the same issuer
  (`auth::oidc`, `auth::routes`).
- **The store** (`store`) keeps sessions and each person's credentials per
  MCP, sealed with XChaCha20-Poly1305 and bound to their row (`crypto`).
- **Backend questions** (`backend`) are GraphQL asked of an MCP with the
  person's credentials, over plain HTTP or the MCP's `graphql` tool:
  whether the credentials work (`backend::verify`), a setting's choices
  (`backend::choices`), and telling the backend a setting changed
  (`backend::sync`). Replies are read into typed structs.

## The web UI

Server-rendered HTML, patched by [Datastar](https://data-star.dev). Markup
comes from askama templates and the components of
[koan-kit-web](https://github.com/radiosilence/koan/tree/main/koan-kit-web);
the glue between them is
[askama_datastar](https://github.com/radiosilence/askama-datastar).

```
src/web.rs                         the route table, and the helpers every answer uses
src/web/assets.rs                  the stylesheet, the script, Datastar and the typeface
src/web/asked.rs                   questions asked of backends, owned by the server
src/web/page.rs                    the Page shape, the layout, and serve
src/web/page/sign_in.rs            `/`
src/web/page/dashboard.rs          `/dashboard`, and signing out
src/web/page/dashboard/panel.rs    one MCP's panel; saving and disconnecting
src/web/page/dashboard/badge.rs    its connection badge, which loads its answer
src/web/page/dashboard/setting.rs  a setting, which loads its choices; saving it
src/web/escaping.rs                the escaping guard
templates/layout.html, layout/     the document, its title and main
templates/<page>/page.html         each page's body
templates/dashboard/<region>.html  the regions the dashboard sends again
styles/app.css                     the stylesheet's source, compiled to assets/app.css
```

A page's module and its template directory carry the name the UI gives it,
and a region of a page is a submodule of it, with its template beside the
page's.

### The route table

`web::routes` declares every route the browser uses in one place:

- **Pages**: `GET /` and `GET /dashboard`, each `page::serve::<Page>`.
- **Actions and regions**: what a page posts to or loads in place, each a
  handler in the module of the region it answers for, its path a constant
  beside it (`panel::CREDENTIALS`, `badge::STATUS`, `setting::SETTING`). They
  sit behind `require_datastar_on_post`, so a POST Datastar did not send is
  refused, and take the session through `FragmentSession`, which answers 401
  rather than redirecting.
- **Sign-in**: `/login` and `/auth/callback`, rate limited per client address.
- **Assets**: the stylesheet under a fingerprinted address, Datastar
  from askama_datastar and the typeface from the kit.

### Pages

Every page implements `page::Page`: its title, what it reads from the request
(any axum extractor, such as `PageSession`), and its body, or the address it
should go to instead. `page::serve` draws a page whole for a browser, and in
place for Datastar: the body morphed into `main`, the title, and the address
as `$url`, which the layout's `kit::Navigation` keeps the address bar on. Signing out answers `askama_datastar::go("/")` with the
cookies cleared.

### No live stream

Unlike slsk-mcp, no page holds a stream open. Nothing on the dashboard changes
except by the person's own action, whose answer redraws what it changed, and
the gateway runs as several replicas, where a stream on one would not hear an
answer fetched by another. What only a backend knows is a region the page
loads once it is up instead: the badge (`kit::Status::loads`) and each
setting (`kit::Field::loads`). So a slow backend holds neither the page
nor the rest of the dashboard.

### Backend questions

The regions never ask a backend themselves. They ask `asked::Questions`, an
`asked::Asked` per kind of question, which asks once however many requests
want the answer at the same moment, on a task of its own so a request that
gives up does not cancel it, and remembers a definite answer for a minute. A
failure (a timeout, a backend that could not say) is shared with the
requests waiting on it and then forgotten, so a reload asks again. Each
replica remembers its own answers, so N replicas ask a question up to N times
a minute, and a refusal (a no-auth MCP's "backend signed out") can stand for
up to a minute after the backend recovers. A page
drawn while an answer stands draws it at once rather than loading it. The key
names everything the answer depends on, the time the credentials were last
saved included, so saving them is a new question on every replica without
anything being told to forget.

### Actions

An action changes something and answers with as little as it can:

- Saving the credential form answers with the panel redrawn and a flash. The
  panel replaces the old one rather than morphing into it, since a morph keeps
  what was typed into an input the server left empty, a secret included. A
  complaint (a required field left empty) is a flash alone, so the form keeps
  what the person typed.
- Changing a setting answers with a flash: the select already shows the
  choice.
- Disconnecting is asked first in the page's one `kit::Confirm`, and answers
  with the panel redrawn, a flash, and `kit::Confirm::close`.

The layout's flash region (`kit::Flash::region().failed(…).unreachable(…)
.sign_in("/login")`) turns a request that failed into a flash, and sends a
session that has ended to the sign-in, since Datastar drops the body of a
failed response. The page carries no script of its own.

### Templates and markup

Every view is a struct deriving `Template` with a file under `templates/`;
there are no inline template sources. A view embeds another as
`askama_datastar::Html` (rendered with `Draw::draw`), which askama writes as
markup, and escapes every other value. Nothing builds markup with `format!`,
nothing calls `Html::trusted`, and no template uses `|safe`
(`assets::tests::no_template_opts_out_of_escaping`). A wrapper is a kit
component with a slot (`{% filter kit::wrap(…) %}`).

Datastar expressions are constants. A URL a region loads from goes in a
`data-url` attribute, escaped, and the expression reads `el.dataset.url`. The
one signal is `url`, the address shown.

### Patch targets

Datastar morphs a region by the id of its top-level element, so ids exist only
where the server sends a region again, each built by one function on its view
that both the page and the region use:

| Where | Ids |
|---|---|
| Layout | `title`, `main`, and the kit's `flash` |
| Dashboard | each MCP's panel `panel-<id>` (`Panel::id_of`), its badge `badge-<id>` (`Badge::id_of`), each setting `setting-<id>.<field>` (`Setting::id_of`) |

Each kind of region has its own prefix, and `.`, which no MCP or field id may
hold, parts an MCP from a field, so no two regions' ids can meet. Each
region's tests check that every state it draws carries its id, and the guard
that no id appears twice on the dashboard. A field the kit draws takes an id
of its own (`credentials-<id>.<field>`, `setting-<id>.<field>-field`), since
the kit builds its label's and note's ids from it, and the same form repeats
once per MCP.

### The kit

Every control, status, notice, flash, empty state, disclosure and question is
a koan-kit-web component; templates add layout utilities only. The stylesheet
is compiled from the kit's source in the crate `Cargo.lock` names (`just
css`, through the `styles/kit` link `just kit` makes), so the markup and the
rules that draw it come from one release, and `kit-check` holds the templates
and code to the kit. What the kit cannot yet express is tracked on
radiosilence/koan#2365.

### The guard

`web::escaping` builds the real routes against a Postgres
(`TEST_DATABASE_URL`) and a fake backend it serves, fills every field the
person, the registry or a backend controls with markup-shaped text, and
requests every page (whole and in place), every region and every action's
answer, signed in, through `askama_datastar::escaping::send`. `askama_datastar::escaping::Guard`, told the hostile text, the
templates and the elements the kit writes, finds markup of the templates' own
escaped, text escaped twice, or text left as markup. A new page, region or
field belongs in it. CI runs it against a Postgres service; without a
database it is skipped locally and refused in CI.
