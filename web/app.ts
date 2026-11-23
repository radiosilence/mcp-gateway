// `datastar` is left out of the bundle and served as askama_datastar's copy;
// main.rs names its address in place of the specifier (`APP_JS_BODY`).
import { action, type FetchDetail } from "datastar";

// `data-on:click="@copy(el.value)"`: the button carries what it copies.
action({
  name: "copy",
  async apply({ el }, text: string) {
    el.dataset.label ??= el.textContent ?? "";
    try {
      await navigator.clipboard.writeText(text);
      el.textContent = "Copied";
    } catch {
      el.textContent = "Copy failed";
    }
    setTimeout(() => {
      el.textContent = el.dataset.label ?? "";
    }, 1200);
  },
});

// Datastar drops the body of a non-200 response and retries network failures
// silently, so both are surfaced here: an expired session goes to the login,
// anything else gets a line at the top of the page until the next request.
document.addEventListener("datastar-fetch", (event) => {
  const { type, argsRaw } = (event as CustomEvent<FetchDetail>).detail;
  const flash = document.getElementById("flash");
  if (type === "started") {
    if (flash) flash.hidden = true;
    return;
  }
  if (type === "error" && argsRaw.status === "401") {
    location.assign("/login");
    return;
  }
  if (!flash) return;
  if (type === "error") {
    flash.textContent = `Something went wrong (${argsRaw.status}). Try again.`;
  } else if (type === "retries-failed") {
    flash.textContent = "The gateway could not be reached. Try again.";
  } else {
    return;
  }
  flash.hidden = false;
});
