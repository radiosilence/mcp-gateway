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

// Settings whose choices come from the backend: the page renders a placeholder,
// the options arrive once, and picking one saves immediately — there is no form
// to submit and nothing lost by navigating away.
for (const select of document.querySelectorAll("select[data-options]")) {
  const status = select.parentNode.querySelector("[data-status]");
  const current = select.getAttribute("data-current") ?? "";

  const say = (text, bad) => {
    status.textContent = text ?? "";
    status.className =
      "mt-1 block " +
      (bad
        ? "text-amber-600 dark:text-amber-400"
        : "text-slate-400 dark:text-slate-500");
  };

  // Deliberately not awaited: each backend is asked in parallel and a slow one
  // holds up neither its neighbours nor the rest of this file.
  (async () => {
    try {
      const response = await fetch(select.getAttribute("data-options"), {
        credentials: "same-origin",
      });
      const options = await response.json();
      if (!Array.isArray(options)) throw new Error(options?.error);

      select.replaceChildren();
      const fallback = options.find((o) => o.isDefault);
      select.append(
        new Option(
          fallback ? `Account default (${fallback.label})` : "Account default",
          "",
        ),
      );
      for (const o of options) {
        if (o.disabled || o.supportsEvents === false) continue;
        const option = new Option(
          o.label + (o.isDefault ? " — account default" : ""),
          o.value,
        );
        if (o.value === current || o.label === current) option.selected = true;
        select.append(option);
      }
      say("");
    } catch (e) {
      say(`Could not load calendars${e.message ? `: ${e.message}` : ""}`, true);
    }
  })();

  select.addEventListener("change", async () => {
    say("Saving…");
    try {
      const response = await fetch(select.getAttribute("data-save"), {
        method: "PATCH",
        credentials: "same-origin",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: `value=${encodeURIComponent(select.value)}`,
      });
      if (!response.ok) throw new Error(await response.text());
      const result = await response.json();
      say(result.message || "Saved", !!result.message);
    } catch (e) {
      say(`Not saved: ${e.message}`, true);
    }
  });
}

// The server sends RFC 3339 so the page reads sensibly without scripting; this
// swaps in the viewer's own locale and timezone.
for (const el of document.querySelectorAll("time[datetime]")) {
  const date = new Date(el.getAttribute("datetime"));
  if (!isNaN(date)) el.textContent = date.toLocaleString();
}
