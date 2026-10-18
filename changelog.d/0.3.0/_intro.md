Released 2026-07-26.

### Changed

- **The page arrives complete.** Settings whose choices come from a backend are
  fetched while the dashboard renders — all of them at once, with a 600ms budget
  — so a connected account shows its real dropdown immediately rather than a
  placeholder that fills in. Anything that misses the budget still loads over
  htmx afterwards, which is also what happens when a backend is refusing, so the
  slow path is the same path rather than a second one that only runs when
  something is wrong.

  The budget is the point: these are network calls to somebody else's server on
  the way to rendering, and the dashboard is what you would use to fix the
  credential that is making them slow. It waits a little for a nicer page and
  never waits long.

- **One definition of a form control.** The placeholder, the control that
  replaces it and the credential inputs all carried the same class list written
  out three times, which is how they came to disagree: a class added to one and
  not the others changed the box model and the swap moved the page. They share a
  macro now.

- **"updated 3 hours ago" instead of a timestamp.** The exact time was rendered
  as RFC 3339 and rewritten by script into the reader's locale, which meant an
  ISO string visibly sitting there until that ran. No request header carries a
  timezone — `Accept-Language` gives a locale, nothing gives a zone — so the
  server cannot format an exact local time at all, and elapsed time sidesteps
  the question. The precise value stays on the element for anything that wants
  it, and there is no longer any script involved.
