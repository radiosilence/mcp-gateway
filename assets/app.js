// Datastar drops the body of a failed response and retries a network failure
// quietly, so both are said here, as a flash over the page. A session that has
// ended goes to the sign-in.
document.addEventListener("datastar-fetch", (event) => {
  const { type, argsRaw } = event.detail;
  const flash = (message) =>
    window.dispatchEvent(new CustomEvent("kit-flash", { detail: { message, bad: true } }));
  if (type === "error" && argsRaw.status === "401") {
    location.assign("/login");
  } else if (type === "error") {
    flash(`Something went wrong (${argsRaw.status}). Try again.`);
  } else if (type === "retries-failed") {
    flash("The gateway could not be reached. Try again.");
  }
});
