import { action, type FetchDetail } from "./vendor/datastar.js";

// `data-on:click="@copy(el.value)"`: the button carries what it copies.
action({
  name: "copy",
  async apply({ el }, text: string) {
    await navigator.clipboard.writeText(text);
    const label = el.textContent;
    el.textContent = "Copied";
    setTimeout(() => {
      el.textContent = label;
    }, 1200);
  },
});

// Datastar drops the body of a non-200 response, so failures are surfaced
// here: an expired session goes to the login, anything else gets a line at the
// top of the page.
document.addEventListener("datastar-fetch", (event) => {
  const { type, argsRaw } = (event as CustomEvent<FetchDetail>).detail;
  if (type !== "error") return;
  if (argsRaw.status === "401") {
    location.assign("/login");
    return;
  }
  const flash = document.getElementById("flash");
  if (!flash) return;
  flash.textContent = `Something went wrong (${argsRaw.status ?? "network error"}). Try again.`;
  flash.hidden = false;
});
