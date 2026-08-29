// Datastar is loaded by its own <script type="module"> beside this one rather
// than imported here, which is how mariastew loads it too — it installs itself
// on the document and exports nothing worth holding.
//
// A module, so it runs after parsing and needs no ready handler.

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
