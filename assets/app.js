// htmx, vendored. Imported rather than script-tagged so there is one entry
// point and no global; an absolute specifier because a bare one would need
// an import map, and those are inline script the CSP refuses.
import "/assets/htmx.esm.min.js";

// Loaded as a module, so it runs after parsing and needs no ready handler.

// Copy buttons: the value rides on the element, so one listener covers however
// many the page renders.
document.addEventListener("click", async (e) => {
  const button = e.target.closest("[data-copy]");
  if (!button) return;
  await navigator.clipboard.writeText(button.getAttribute("data-copy"));
  const previous = button.textContent;
  button.textContent = "Copied";
  setTimeout(() => {
    button.textContent = previous;
  }, 1200);
});

// Destructive forms confirm first. As an attribute rather than an inline
// handler so the page needs no script-src exception, and so a registry name
// containing a quote cannot break out of a JavaScript string literal.
document.addEventListener("submit", (e) => {
  const message = e.target.getAttribute("data-confirm");
  if (message && !confirm(message)) e.preventDefault();
});

// The server sends RFC 3339 so the page reads sensibly without scripting; this
// swaps in the viewer's own locale and timezone.
for (const el of document.querySelectorAll("time[datetime]")) {
  const date = new Date(el.getAttribute("datetime"));
  if (!isNaN(date)) el.textContent = date.toLocaleString();
}
