Released 2026-07-26.

### Fixed

- **A secret could never be shown as set.** The stored values were only fetched
  when an MCP had at least one visible field, because until now they were only
  needed to prefill one. Every MCP holding nothing but secrets — Fastmail, TfL —
  therefore rendered as though nothing were stored, which is precisely the case
  the marker exists for.

- **A backend that declares no check now reads as connected.** It was showing
  "configured", which implies a doubt there are no grounds for: nothing was
  asked, so nothing came back unanswered. TfL authenticates nothing and a key
  there only raises a rate limit, so there is nothing to be unsure about. A
  declared check that could not be completed still reads as configured, because
  that one genuinely is unknown.

- **A failed lookup is a sentence, not a clipped dropdown.** The reason came
  back inside a `<select>`, which cannot wrap, so it was cut off mid-word — and
  offering a control to choose from when there is nothing to choose is its own
  small lie.

- A check that cannot be completed now says so in the log. It returned unknown
  silently, which is unhelpful precisely when something is wrong.
