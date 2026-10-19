// Small client helpers. HTMX does the heavy lifting; this covers dialogs and toasts.
import { CATEGORIES } from "./emoji";

type ToastKind = "success" | "error" | "info";
type ToastDetail = { kind?: ToastKind; message: string };

function showToast({ kind = "info", message }: ToastDetail) {
  let region = document.querySelector<HTMLElement>(".toast-region");
  if (!region) {
    region = document.createElement("div");
    region.className = "toast-region";
    region.setAttribute("role", "status");
    document.body.append(region);
  }
  const toast = document.createElement("div");
  toast.className = `toast toast-${kind}`;
  toast.textContent = message;
  region.append(toast);
  window.setTimeout(() => toast.remove(), 3500);
}

document.addEventListener("click", (event) => {
  const target = event.target as Element | null;
  const opener = target?.closest<HTMLElement>("[data-dialog-open]");
  if (opener) {
    document.getElementById(opener.dataset.dialogOpen ?? "")?.closest("dialog")?.showModal();
    return;
  }
  const closer = target?.closest<HTMLElement>("[data-dialog-close]");
  if (closer) closer.closest("dialog")?.close();
  // Clicking the backdrop of a dialog closes it.
  if (target instanceof HTMLDialogElement) target.close();
});

// Server responses send `HX-Trigger: {"toast": {...}}`; HTMX dispatches it as an event.
document.body.addEventListener("toast", (event) => showToast((event as CustomEvent<ToastDetail>).detail));

// Forms marked data-close-on-success close their dialog and reset after a successful HTMX request.
document.body.addEventListener("htmx:afterRequest", (event) => {
  const { elt, successful } = (event as CustomEvent<{ elt: Element; successful: boolean }>).detail;
  if (!successful || !(elt instanceof HTMLFormElement) || !elt.hasAttribute("data-close-on-success")) return;
  elt.reset();
  elt.closest("dialog")?.close();
});

// Flash messages rendered by the server on full page loads.
document.querySelectorAll<HTMLElement>("[data-flash]").forEach((el) => {
  showToast({ kind: (el.dataset.flash as ToastKind) || "info", message: el.textContent ?? "" });
  el.remove();
});

// Stale tabs. Every app page says which organisation it was drawn for
// (`<body data-org-id>`), and every request carries it: HTMX through the body's
// hx-headers, plain forms through a hidden `_org`, raw fetch calls through
// orgHeaders(). After a switch in another tab the server refuses this tab's
// requests with 409 and `HX-Redirect` (or 303 for a plain form) to Home.
const pageOrgId = () => document.body.dataset.orgId ?? "";

// Never HX-Request: the server answers that with HX-Redirect instead of a
// redirect, which changes what these calls see (secretRefused relies on it).
function orgHeaders(): Record<string, string> {
  const id = pageOrgId();
  return id ? { "X-Org-Id": id } : {};
}

// Plain urlencoded forms. HTMX forms send the header, the chat composer goes
// over its socket, the unlock form is a fetch, and multipart bodies are never
// read for `_org`, so all of those are left alone.
document.addEventListener(
  "submit",
  (event) => {
    const form = event.target;
    const id = pageOrgId();
    if (!(form instanceof HTMLFormElement) || !id || form.method !== "post") return;
    if (["hx-post", "ws-send", "data-unlock-form"].some((a) => form.hasAttribute(a)) || form.enctype === "multipart/form-data") return;
    let field = form.querySelector<HTMLInputElement>('input[name="_org"]');
    if (!field) {
      field = document.createElement("input");
      field.type = "hidden";
      field.name = "_org";
      form.append(field);
    }
    field.value = id;
  },
  true,
);

// Text being typed, or a document upload waiting or under way. A tab holding
// either doesn't move when it goes stale; it shows one banner instead.
function hasUnsavedWork(): boolean {
  if (currentUpload || uploadQueue.length) return true;
  const skip = ["hidden", "password", "search", "checkbox", "radio", "file", "submit", "button"];
  return Array.from(document.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>("input, textarea")).some(
    (field) =>
      !(field instanceof HTMLInputElement && (skip.includes(field.type) || field.dataset.filter !== undefined)) &&
      field.value !== field.defaultValue &&
      !!field.form &&
      // Drawn: in a visible form, or a form inside an open dialog.
      field.getClientRects().length > 0,
  );
}

// Follows a refusal to Home, unless the tab holds unsaved work: then the
// banner shows in place and the text stays where it is. Returns whether the
// refusal was held.
let staleHome = "";
function goHomeOrHold(home: string): boolean {
  staleHome = home;
  if (!document.querySelector("[data-stale-banner]") && !hasUnsavedWork()) {
    window.location.href = home;
    return false;
  }
  if (document.querySelector("[data-stale-banner]")) return true;
  const banner = document.querySelector<HTMLTemplateElement>("#stale-banner-template")?.content.firstElementChild?.cloneNode(true);
  if (!(banner instanceof HTMLElement)) {
    window.location.href = home;
    return false;
  }
  // Inside an open sheet, so it shows above it; otherwise at the top of the page.
  const host = document.querySelector("dialog[open] .sheet-body") ?? document.querySelector("main.page") ?? document.body;
  host.prepend(banner);
  return true;
}

// For raw fetch calls: true when the response was a stale-tab refusal, which
// has been dealt with. Callers check it first.
function staleRefusal(res: Response): boolean {
  const home = res.status === 409 ? res.headers.get("HX-Redirect") : null;
  if (!home) return false;
  goHomeOrHold(home);
  return true;
}

// HTMX checks the HX-Redirect before the status, so cancelling here is the
// only way to keep the tab where it is (htmx 2.0.11).
document.body.addEventListener("htmx:beforeOnLoad", (event) => {
  const xhr = (event as CustomEvent<{ xhr: XMLHttpRequest }>).detail.xhr;
  const home = xhr.status === 409 ? xhr.getResponseHeader("HX-Redirect") : null;
  if (home && goHomeOrHold(home)) event.preventDefault();
});

// "Back to Acme": switch this browser back to the page's own org. A manual
// redirect, so the tab stays put; the browser still stores the cookie.
document.addEventListener("click", (event) => {
  const target = event.target as Element | null;
  if (target?.closest("[data-stale-home]")) {
    window.location.href = staleHome || "/dashboard";
    return;
  }
  const back = target?.closest<HTMLButtonElement>("[data-stale-back]");
  if (!back) return;
  const name = back.dataset.orgName ?? "";
  back.disabled = true;
  fetch(`/orgs/${pageOrgId()}/switch`, { method: "POST", redirect: "manual", credentials: "same-origin" })
    .then((res) => {
      if (res.type === "opaqueredirect") {
        back.closest("[data-stale-banner]")?.remove();
        showToast({ message: `You're back in ${name}` });
        void startNextUpload();
        return;
      }
      showToast({ kind: "error", message: res.status === 404 ? `You can't open ${name} any more.` : `Couldn't switch to ${name}. Try again.` });
    })
    .catch(() => showToast({ kind: "error", message: `Couldn't switch to ${name}. Check your connection and try again.` }))
    .finally(() => (back.disabled = false));
});

// Asks the server whether this tab is still in the browser's org. A stale tab
// gets the refusal and moves (or holds).
function probeOrg() {
  if (pageOrgId()) void htmx.ajax("GET", "/orgs/current", { swap: "none" });
}
window.addEventListener("pageshow", (event) => {
  if (event.persisted) probeOrg();
});

// After a switch or a stale-tab move, drop ?switched= and ?from= from Home's
// address, so a reload doesn't repeat the toast or the notice.
{
  const here = new URL(window.location.href);
  if (here.searchParams.has("switched") || here.searchParams.has("from")) {
    here.searchParams.delete("switched");
    here.searchParams.delete("from");
    history.replaceState(history.state, "", `${here.pathname}${here.search}${here.hash}`);
  }
}
document.addEventListener("click", (event) => {
  (event.target as Element | null)?.closest("[data-dismiss-notice]")?.closest("[data-stale-notice]")?.remove();
});

// Chat: Enter sends, Shift+Enter adds a new line.
document.addEventListener("keydown", (event) => {
  const target = event.target;
  if (!(target instanceof HTMLTextAreaElement) || !target.hasAttribute("data-enter-submits")) return;
  if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
  event.preventDefault();
  if (target.value.trim()) target.form?.requestSubmit();
});

// Keep feeds marked data-autoscroll pinned to the newest message. Coming back
// from a thread (#message-N) shows that message instead; one no longer in the
// feed (older than what it shows) falls back to the end.
function scrollToEnd(el: Element) {
  el.scrollTop = el.scrollHeight;
}
document.querySelectorAll("[data-autoscroll]").forEach((feed) => {
  const target = /^#message-\d+$/.test(location.hash) ? feed.querySelector(location.hash) : null;
  if (target) target.scrollIntoView({ block: "center" });
  else scrollToEnd(feed);
});
document.body.addEventListener("htmx:afterSwap", (event) => {
  const target = (event as CustomEvent<{ target: Element }>).detail.target;
  const feed = target.closest("[data-autoscroll]") ?? target.querySelector("[data-autoscroll]");
  if (feed) scrollToEnd(feed);
});

// Live chat (HTMX ws extension): clear the composer once a message is sent,
// and reload the feed after a reconnect so nothing sent meanwhile is missed.
document.body.addEventListener("htmx:wsAfterSend", (event) => {
  const elt = (event as CustomEvent<{ elt: Element }>).detail.elt;
  const form = elt instanceof HTMLFormElement ? elt : elt.closest("form");
  form?.reset();
});

// A closed socket may mean a switch in another tab (its reconnect is then
// refused), so ask whether this tab is still in the browser's org.
const reconnecting = new WeakSet<Element>();
document.body.addEventListener("htmx:wsClose", (event) => {
  reconnecting.add((event as CustomEvent<{ elt: Element }>).detail.elt);
  probeOrg();
});
document.body.addEventListener("htmx:wsOpen", (event) => {
  const elt = (event as CustomEvent<{ elt: Element }>).detail.elt;
  if (!reconnecting.has(elt)) return;
  reconnecting.delete(elt);
  const url = elt.getAttribute("data-chat-feed-url");
  if (url) void htmx.ajax("GET", url, { target: "#chat-feed", swap: "innerHTML" });
});

// Chat message actions: a tap, click, Enter or Space on a bubble opens its
// sheet (react, who reacted). Taps on a chip, the thread footer or a button do their
// own thing, and a tap that ends a text selection leaves the selection alone.
function openActions(bubble: HTMLElement) {
  const url = bubble.dataset.actionsUrl;
  const dialog = document.querySelector<HTMLDialogElement>("#chat-dialog");
  const body = dialog?.querySelector<HTMLElement>("#chat-dialog-body");
  if (!url || !dialog || !body) return;
  const failed = () => showToast({ kind: "error", message: "Couldn't open that message. Check your connection and try again." });
  fetch(url, { headers: { "HX-Request": "true", ...orgHeaders() }, credentials: "same-origin" })
    .then(async (res) => {
      if (staleRefusal(res)) return;
      if (!res.ok || res.redirected) return failed();
      // The server escapes the message; this is its own fragment.
      body.innerHTML = await res.text();
      dialog.showModal();
    })
    .catch(failed);
}
// A mouse click waits a moment, so a double or triple click selects text
// instead of opening the sheet. Taps open straight away.
let lastPointer = "";
let pendingOpen: number | undefined;
document.addEventListener("pointerdown", (event) => (lastPointer = event.pointerType), true);
function selectingIn(bubble: Element) {
  const selection = getSelection();
  return !!selection && !selection.isCollapsed && bubble.contains(selection.anchorNode);
}
document.addEventListener("click", (event) => {
  const target = event.target as Element | null;
  const bubble = target?.closest<HTMLElement>(".bubble[data-actions-url]");
  if (!bubble || target?.closest("a, button, .react, .thread-foot")) return;
  window.clearTimeout(pendingOpen);
  if (event.detail > 1 || selectingIn(bubble)) return;
  if (lastPointer !== "mouse") return openActions(bubble);
  pendingOpen = window.setTimeout(() => {
    if (!selectingIn(bubble)) openActions(bubble);
  }, 250);
});
document.addEventListener("keydown", (event) => {
  const bubble = event.target;
  if (!(bubble instanceof HTMLElement) || !bubble.matches(".bubble[data-actions-url]")) return;
  if (event.key !== "Enter" && event.key !== " ") return;
  event.preventDefault();
  openActions(bubble);
});

// Reactions: a chip, a toolbar button or a sheet button sets your reaction. The
// change shows at once; the server's answer then replaces the chips. The order
// matches `reactions::EMOJI`, so a new chip lands where the server puts it.
const REACTIONS = ["👍", "❤️", "😂", "🎉", "👀", "✅"];
const reacting = new Set<string>();

function chipFor(messageId: string, emoji: string) {
  const row = document.getElementById(`reacts-${messageId}`);
  return Array.from(row?.querySelectorAll<HTMLButtonElement>(".react") ?? []).find((b) => b.dataset.emoji === emoji);
}

// Shows whether you reacted with `emoji`, adjusting only your own part: the
// count moves by one, a new chip is added, or a chip that reaches 0 goes. Does
// nothing when the page already shows that (say, after a live update).
function showMine(messageId: string, emoji: string, url: string, mine: boolean) {
  const row = document.getElementById(`reacts-${messageId}`);
  const chip = chipFor(messageId, emoji);
  if (!row || (chip?.classList.contains("mine") ?? false) === mine) return;
  if (chip) {
    const count = chip.querySelector(".react-count");
    const next = Number(count?.textContent ?? "0") + (mine ? 1 : -1);
    if (next <= 0) {
      chip.closest(".react-wrap")?.remove();
    } else {
      if (count) count.textContent = String(next);
      chip.classList.toggle("mine", mine);
      chip.setAttribute("aria-pressed", String(mine));
      chip.setAttribute("aria-label", `${emoji} ${next}${mine ? ", you reacted" : ""}`);
    }
  } else if (mine) {
    const wrap = document.createElement("span");
    wrap.className = "react-wrap";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "react mine";
    Object.assign(button.dataset, { react: "", emoji, message: messageId, url });
    button.setAttribute("aria-pressed", "true");
    button.setAttribute("aria-label", `${emoji} 1, you reacted`);
    const face = document.createElement("span");
    face.className = "emoji";
    face.textContent = emoji;
    const count = document.createElement("span");
    count.className = "react-count";
    count.textContent = "1";
    button.append(face, count);
    wrap.append(button);
    const after = Array.from(row.querySelectorAll<HTMLElement>(".react")).find(
      (b) => REACTIONS.indexOf(b.dataset.emoji ?? "") > REACTIONS.indexOf(emoji),
    );
    row.insertBefore(wrap, after?.closest(".react-wrap") ?? null);
  }
  const tool = Array.from(document.querySelectorAll<HTMLElement>(`#tools-${messageId} [data-react]`)).find((b) => b.dataset.emoji === emoji);
  tool?.classList.toggle("mine", mine);
  tool?.setAttribute("aria-pressed", String(mine));
}

// The reaction button that has focus, so a swap that replaces it can put focus back.
type FocusedReaction = { holder: string; emoji: string } | null;
function focusedReaction(): FocusedReaction {
  const el = document.activeElement;
  const holder = el?.closest<HTMLElement>(".reacts, .tool-picks");
  return holder && el instanceof HTMLElement && el.dataset.emoji ? { holder: holder.id, emoji: el.dataset.emoji } : null;
}
// Only when the swap dropped focus to the page, never taking it from elsewhere.
function restoreFocus(was: FocusedReaction) {
  if (!was || document.activeElement !== document.body) return;
  const holder = document.getElementById(was.holder);
  const button = Array.from(holder?.querySelectorAll<HTMLElement>("[data-emoji]") ?? []).find((b) => b.dataset.emoji === was.emoji);
  if (!holder || !button) return;
  // The toolbar only shows once something in its message has focus, so show it to focus it.
  const shown = holder.closest<HTMLElement>(".msg-tools") ?? holder;
  shown.style.display = "flex";
  button.focus();
  shown.style.removeProperty("display");
}

// Replaces each top-level element of `html` with the page's element of the same id.
function swapById(html: string) {
  const template = document.createElement("template");
  template.innerHTML = html;
  Array.from(template.content.children).forEach((el) => {
    const was = focusedReaction();
    if (el.id) document.getElementById(el.id)?.replaceWith(el);
    restoreFocus(was);
  });
}

// Live reaction swaps from the socket keep focus the same way.
let focusBeforeMessage: FocusedReaction = null;
document.body.addEventListener("htmx:wsBeforeMessage", () => (focusBeforeMessage = focusedReaction()));
document.body.addEventListener("htmx:wsAfterMessage", () => restoreFocus(focusBeforeMessage));

document.addEventListener("click", (event) => {
  const button = (event.target as Element | null)?.closest<HTMLElement>("[data-react]");
  const { message, emoji, url } = button?.dataset ?? {};
  if (!button || !message || !emoji || !url) return;
  button.closest("dialog")?.close();
  // One request at a time per reaction; a tap meanwhile is ignored.
  const key = `${message} ${emoji}`;
  if (reacting.has(key)) return;
  reacting.add(key);
  // Send the state wanted, not a toggle, so a retried request changes nothing.
  const on = !(chipFor(message, emoji)?.classList.contains("mine") ?? false);
  showMine(message, emoji, url, on);
  fetch(url, {
    method: "POST",
    headers: { "HX-Request": "true", ...orgHeaders() },
    body: new URLSearchParams({ emoji, on: on ? "1" : "0" }),
    credentials: "same-origin",
  })
    .then(async (res) => {
      if (staleRefusal(res)) return showMine(message, emoji, url, !on);
      if (!res.ok || res.redirected) throw new Error(`status ${res.status}`);
      swapById(await res.text());
    })
    .catch(() => {
      showMine(message, emoji, url, !on);
      showToast({ kind: "error", message: `Couldn't ${on ? "add" : "remove"} your reaction. Check your connection and try again.` });
    })
    .finally(() => reacting.delete(key));
});

// Escape on a thread goes back to its conversation, on desktop. Not while
// typing (a field with text), with a sheet open, or with the mention menu
// open, which handles Escape itself before this runs.
document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) return;
  const back = document.querySelector<HTMLAnchorElement>("[data-thread-back]");
  if (!back || document.querySelector("dialog[open]") || !matchMedia("(min-width: 900px)").matches) return;
  const field = document.activeElement;
  if ((field instanceof HTMLTextAreaElement || field instanceof HTMLInputElement) && field.value !== "") return;
  location.href = back.href;
});

// Search-as-you-type for lists: <input data-filter="#list"> hides rows whose
// data-filter-text does not contain the query.
document.addEventListener("input", (event) => {
  const input = event.target;
  if (!(input instanceof HTMLInputElement) || !input.dataset.filter) return;
  const query = input.value.trim().toLowerCase();
  document.querySelectorAll<HTMLElement>(`${input.dataset.filter} [data-filter-text]`).forEach((row) => {
    row.hidden = query !== "" && !(row.dataset.filterText ?? "").toLowerCase().includes(query);
  });
});

// Textareas marked data-autogrow grow with their content (up to their CSS max-height).
function autogrow(el: HTMLTextAreaElement) {
  el.style.height = "auto";
  el.style.height = `${el.scrollHeight}px`;
}
document.addEventListener("input", (event) => {
  if (event.target instanceof HTMLTextAreaElement && event.target.hasAttribute("data-autogrow")) autogrow(event.target);
});
document.body.addEventListener("htmx:wsAfterSend", () => {
  document.querySelectorAll<HTMLTextAreaElement>("textarea[data-autogrow]").forEach(autogrow);
});


// <button data-toggle="#id"> shows or hides an element and focuses its first input.
// Buttons that carry aria-expanded have it kept in step.
document.addEventListener("click", (event) => {
  const button = (event.target as Element | null)?.closest<HTMLElement>("[data-toggle]");
  if (!button) return;
  const target = document.querySelector<HTMLElement>(button.dataset.toggle ?? "");
  if (!target) return;
  target.classList.toggle("hidden");
  if (button.hasAttribute("aria-expanded")) button.setAttribute("aria-expanded", String(!target.classList.contains("hidden")));
  if (!target.classList.contains("hidden")) target.querySelector<HTMLInputElement>("input:not([type=hidden])")?.focus();
});

// Escape inside the task sheet's modal <dialog> would also close the sheet.
// A popup that handles Escape itself claims it on keydown; the dialog's
// cancel, which follows that keydown, is then stopped too.
let escapeClaimed = false;
function claimEscape(event: KeyboardEvent) {
  event.preventDefault();
  escapeClaimed = true;
  window.setTimeout(() => (escapeClaimed = false));
}
document.addEventListener(
  "cancel",
  (event) => {
    if (escapeClaimed) event.preventDefault();
  },
  true,
);

// Task notes: the smiley-plus button opens the six quick picks; Escape or a
// click anywhere else closes them. A failed reaction shows a toast; a 422
// brings its own through HX-Trigger.
function closeQuickReact(bar: HTMLElement) {
  bar.classList.add("hidden");
  document.querySelector(`[data-toggle="#${bar.id}"]`)?.setAttribute("aria-expanded", "false");
}
document.addEventListener("click", (event) => {
  const target = event.target as Element | null;
  const opener = target?.closest<HTMLElement>(".note-add")?.dataset.toggle;
  document.querySelectorAll<HTMLElement>(".quick-react:not(.hidden)").forEach((bar) => {
    if ((target && bar.contains(target)) || opener === `#${bar.id}`) return;
    closeQuickReact(bar);
  });
});
document.addEventListener(
  "keydown",
  (event) => {
    if (event.key !== "Escape" || event.isComposing) return;
    const bar = document.querySelector<HTMLElement>(".quick-react:not(.hidden)");
    if (!bar) return;
    claimEscape(event);
    closeQuickReact(bar);
    document.querySelector<HTMLElement>(`[data-toggle="#${bar.id}"]`)?.focus();
  },
  true,
);
for (const name of ["htmx:responseError", "htmx:sendError"]) {
  document.body.addEventListener(name, (event) => {
    const detail = (event as CustomEvent<{ elt?: Element; xhr?: XMLHttpRequest }>).detail;
    if (!detail.elt?.closest("[data-note-react]") || detail.xhr?.status === 422) return;
    showToast({ kind: "error", message: "Couldn't save your reaction. Try again." });
  });
}

// <button data-copy="#input"> copies that input's value. `data-copy-message`
// changes the toast (default "Link copied").
document.addEventListener("click", (event) => {
  const button = (event.target as Element | null)?.closest<HTMLElement>("[data-copy]");
  if (!button) return;
  const input = document.querySelector<HTMLInputElement>(button.dataset.copy ?? "");
  if (!input) return;
  void navigator.clipboard.writeText(input.value).then(
    () => showToast({ kind: "success", message: button.dataset.copyMessage ?? "Link copied" }),
    () => input.select(),
  );
});

// Responses that delete something send "close-dialogs" to close any open panel.
document.body.addEventListener("close-dialogs", () => {
  document.querySelectorAll("dialog[open]").forEach((dialog) => (dialog as HTMLDialogElement).close());
});

// Task board: drag a card to another column to change its status. Cards stay
// links, so keyboards and phones open the task and change the status there.
let dragged: HTMLElement | null = null;
document.addEventListener("dragstart", (event) => {
  const card = (event.target as Element | null)?.closest<HTMLElement>(".task-card");
  if (!card || !event.dataTransfer) return;
  dragged = card;
  event.dataTransfer.effectAllowed = "move";
  event.dataTransfer.setData("text/plain", card.dataset.taskId ?? "");
  card.classList.add("is-dragging");
});
document.addEventListener("dragend", () => {
  dragged?.classList.remove("is-dragging");
  dragged = null;
  document.querySelectorAll(".is-drop-target").forEach((el) => el.classList.remove("is-drop-target"));
});
document.addEventListener("dragover", (event) => {
  const cell = (event.target as Element | null)?.closest<HTMLElement>("[data-drop-status]");
  if (!cell || !dragged) return;
  event.preventDefault();
  document.querySelectorAll(".is-drop-target").forEach((el) => el !== cell && el.classList.remove("is-drop-target"));
  cell.classList.add("is-drop-target");
});
document.addEventListener("drop", (event) => {
  const cell = (event.target as Element | null)?.closest<HTMLElement>("[data-drop-status]");
  const card = dragged;
  if (!cell || !card) return;
  event.preventDefault();
  cell.classList.remove("is-drop-target");
  const status = cell.dataset.dropStatus ?? "";
  if (status === card.dataset.status) return;
  // Move the card straight away; the board refreshes from the server after the save.
  cell.insertBefore(card, cell.querySelector(".cell-add"));
  card.dataset.status = status;
  void htmx.ajax("POST", `/tasks/${card.dataset.taskId}/status`, { values: { status }, swap: "none" });
});

// Collapsed swimlanes stay collapsed, across board refreshes and visits.
const LANES_KEY = "collab:collapsed-lanes";
function collapsedLanes(): string[] {
  try {
    return JSON.parse(localStorage.getItem(LANES_KEY) ?? "[]") as string[];
  } catch {
    return [];
  }
}
function applyCollapsedLanes(root: ParentNode) {
  const collapsed = collapsedLanes();
  root.querySelectorAll<HTMLElement>("[data-lane]").forEach((lane) => {
    lane.querySelector("[data-lane-toggle]")?.setAttribute("aria-expanded", String(!collapsed.includes(lane.dataset.lane ?? "")));
  });
}
document.addEventListener("click", (event) => {
  const toggle = (event.target as Element | null)?.closest<HTMLElement>("[data-lane-toggle]");
  const key = toggle?.closest<HTMLElement>("[data-lane]")?.dataset.lane;
  if (!toggle || !key) return;
  const open = toggle.getAttribute("aria-expanded") !== "false";
  toggle.setAttribute("aria-expanded", String(!open));
  const collapsed = collapsedLanes().filter((k) => k !== key);
  if (open) collapsed.push(key);
  try {
    localStorage.setItem(LANES_KEY, JSON.stringify(collapsed));
  } catch {
    // Private browsing can refuse storage; collapsing still works for this view.
  }
});
applyCollapsedLanes(document);
document.body.addEventListener("htmx:afterSwap", (event) => {
  applyCollapsedLanes((event as CustomEvent<{ target: Element }>).detail.target.parentElement ?? document);
});

// Notifications: the server pushes a `notify` event when this person gets a new
// one, and everything listening for "notifications-changed" reloads. The
// browser reconnects on its own; badges also poll every 60s as a fallback.
if (document.querySelector('[hx-get^="/notifications/unread"]')) {
  const events = new EventSource("/notifications/stream");
  events.addEventListener("notify", () => htmx.trigger(document.body, "notifications-changed"));
  // A switch in another tab closes the stream; when it comes back, check
  // this tab is still in the browser's org.
  let streamOpened = false;
  events.addEventListener("open", () => {
    if (streamOpened) probeOrg();
    streamOpened = true;
  });
  window.addEventListener("pagehide", () => events.close());
}

// @ mentions: a textarea with data-mentions="raj,maya" offers matching
// teammates after "@". Arrow keys move, Enter or Tab picks, Escape closes.
// The server finds mentions on its own, so typing a name in full also works.
const mentionMenu = document.createElement("ul");
mentionMenu.className = "mention-menu";
mentionMenu.setAttribute("role", "listbox");
mentionMenu.setAttribute("aria-label", "Teammates");
mentionMenu.hidden = true;
let mentionField: HTMLTextAreaElement | null = null;
let mentionActive = 0;

function mentionQuery(field: HTMLTextAreaElement) {
  const before = field.value.slice(0, field.selectionStart);
  const match = /(^|[^A-Za-z0-9_.@])@([A-Za-z0-9]{0,30})$/.exec(before);
  return match ? { start: before.length - match[2].length - 1, query: match[2].toLowerCase() } : null;
}

function closeMentions() {
  mentionMenu.hidden = true;
  mentionField?.removeAttribute("aria-activedescendant");
  mentionField = null;
}

function markActiveMention() {
  Array.from(mentionMenu.children).forEach((li, i) => li.setAttribute("aria-selected", String(i === mentionActive)));
  mentionField?.setAttribute("aria-activedescendant", `mention-option-${mentionActive}`);
}

// "@everyone" with an icon and who it tells. The hint is text, never HTML.
const USERS_ICON =
  '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><path d="M16 3.128a4 4 0 0 1 0 7.744"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><circle cx="9" cy="7" r="4"/></svg>';
function everyoneOption(li: HTMLLIElement, hint: string) {
  const icon = document.createElement("span");
  icon.className = "mm-icon";
  icon.innerHTML = USERS_ICON;
  const text = document.createElement("span");
  text.className = "mm-text";
  const label = document.createElement("b");
  label.textContent = "@everyone";
  text.append(label);
  if (hint) {
    const small = document.createElement("span");
    small.className = "mm-hint";
    small.textContent = hint;
    text.append(small);
  }
  li.append(icon, text);
}

function showMentions(field: HTMLTextAreaElement) {
  const found = mentionQuery(field);
  const names = (field.dataset.mentions ?? "").split(",").filter(Boolean);
  const matches = found ? names.filter((n) => n.toLowerCase().startsWith(found.query)).slice(0, 6) : [];
  if (matches.length === 0) return closeMentions();
  mentionField = field;
  mentionActive = 0;
  mentionMenu.replaceChildren(
    ...matches.map((name, i) => {
      const li = document.createElement("li");
      li.id = `mention-option-${i}`;
      li.setAttribute("role", "option");
      li.dataset.name = name;
      if (name === "everyone") everyoneOption(li, field.dataset.everyoneHint ?? "");
      else li.textContent = `@${name}`;
      return li;
    }),
  );
  field.parentElement?.append(mentionMenu);
  markActiveMention();
  mentionMenu.hidden = false;
}

function pickMention(name: string) {
  const field = mentionField;
  const found = field && mentionQuery(field);
  if (!field || !found) return closeMentions();
  const end = field.selectionStart;
  field.value = `${field.value.slice(0, found.start)}@${name} ${field.value.slice(end)}`;
  const caret = found.start + name.length + 2;
  field.setSelectionRange(caret, caret);
  closeMentions();
  field.focus();
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

document.addEventListener("input", (event) => {
  const target = event.target;
  if (target instanceof HTMLTextAreaElement && target.dataset.mentions !== undefined) showMentions(target);
});

// Capture phase, so Enter picks a name instead of sending the chat message.
document.addEventListener(
  "keydown",
  (event) => {
    if (mentionMenu.hidden || event.target !== mentionField || event.isComposing) return;
    const count = mentionMenu.children.length;
    if (event.key === "ArrowDown") mentionActive = (mentionActive + 1) % count;
    else if (event.key === "ArrowUp") mentionActive = (mentionActive + count - 1) % count;
    else if (event.key === "Enter" || event.key === "Tab") {
      const li = mentionMenu.children[mentionActive] as HTMLElement | undefined;
      if (li?.dataset.name) pickMention(li.dataset.name);
    } else if (event.key === "Escape") closeMentions();
    else return;
    event.preventDefault();
    event.stopImmediatePropagation();
    markActiveMention();
  },
  true,
);

// mousedown, not click: picking must happen before the textarea loses focus.
mentionMenu.addEventListener("mousedown", (event) => {
  const li = (event.target as Element).closest<HTMLElement>("[data-name]");
  if (!li?.dataset.name) return;
  event.preventDefault();
  pickMention(li.dataset.name);
});
document.addEventListener("focusout", (event) => {
  if (event.target === mentionField) closeMentions();
});

// Emoji picker: one element, built on first use and moved to the field it
// serves. A popover on desktop. Everything is built with textContent; only
// the constant icons below go through innerHTML.
const SEARCH_ICON =
  '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m21 21-4.34-4.34"/><circle cx="11" cy="11" r="8"/></svg>';
const CLEAR_ICON =
  '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>';
const CLOCK_ICON =
  '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg>';
const RECENT_KEY = "collab:emoji-recent";
const RECENT_MAX = 16;
const GRID_COLUMNS = 8;
// The name shows in labels and the footer; search also uses the keywords.
const EMOJI_NAMES = new Map<string, string>(CATEGORIES.flatMap((c) => c.list.map(([e, name]): [string, string] => [e, name])));
const EMOJI_SEARCH = new Map<string, string[]>(
  CATEGORIES.flatMap((c) => c.list.map(([e, name, keywords = ""]): [string, string[]] => [e, `${name} ${keywords}`.split(" ").filter(Boolean)])),
);

// Recent picks, newest first. No storage (private browsing) or bad JSON means none.
function recentEmoji(): string[] {
  try {
    const list: unknown = JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
    if (!Array.isArray(list)) return [];
    return list.filter((e): e is string => typeof e === "string" && EMOJI_NAMES.has(e)).slice(0, RECENT_MAX);
  } catch {
    return [];
  }
}
function rememberEmoji(emoji: string) {
  try {
    const list = [emoji, ...recentEmoji().filter((e) => e !== emoji)].slice(0, RECENT_MAX);
    localStorage.setItem(RECENT_KEY, JSON.stringify(list));
  } catch {
    // No storage: Recent just stays empty.
  }
}

// Every picker button. type="button", because the picker sits inside the
// chat form and the note form, where a default button would submit them.
function pickerButton(className: string, label: string): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  if (className) button.className = className;
  button.setAttribute("aria-label", label);
  return button;
}

type Picker = {
  root: HTMLElement;
  search: HTMLInputElement;
  clear: HTMLButtonElement;
  tabs: HTMLElement;
  body: HTMLElement;
  footEmoji: HTMLElement;
  footName: HTMLElement;
};
let picker: Picker | null = null;
let pickerField: HTMLTextAreaElement | null = null;
let pickerToggle: HTMLButtonElement | null = null;
// Phones: the picker is docked where the keyboard was. A tap on the picker
// can move the blurred field's cursor (seen in Chrome's touch emulation), so
// while docked the picker keeps the cursor itself.
let pickerDocked = false;
let dockedCaret = 0;
let dockedCaretEnd = 0;

function buildPicker(): Picker {
  const root = document.createElement("div");
  root.id = "emoji-picker";
  root.className = "ep";
  root.setAttribute("role", "dialog");
  root.setAttribute("aria-label", "Emoji");

  const searchRow = document.createElement("div");
  searchRow.className = "ep-search";
  const icon = document.createElement("span");
  icon.className = "ep-search-icon";
  icon.innerHTML = SEARCH_ICON;
  // No name, so neither the chat socket nor the note form sends it.
  const search = document.createElement("input");
  search.type = "search";
  search.placeholder = "Search emoji";
  search.setAttribute("aria-label", "Search emoji");
  search.autocomplete = "off";
  const clear = pickerButton("ep-clear", "Clear search");
  clear.innerHTML = CLEAR_ICON;
  clear.hidden = true;
  searchRow.append(icon, search, clear);

  const tabs = document.createElement("div");
  tabs.className = "ep-tabs";
  const recentTab = pickerButton("ep-tab", "Recent");
  recentTab.dataset.section = "recent";
  recentTab.innerHTML = CLOCK_ICON;
  tabs.append(recentTab);
  for (const category of CATEGORIES) {
    const tab = pickerButton("ep-tab", category.label);
    tab.dataset.section = category.id;
    const glyph = document.createElement("span");
    glyph.className = "emoji";
    glyph.setAttribute("aria-hidden", "true");
    glyph.textContent = category.icon;
    tab.append(glyph);
    tabs.append(tab);
  }

  const body = document.createElement("div");
  body.className = "ep-body";

  const foot = document.createElement("div");
  foot.className = "ep-foot";
  foot.setAttribute("aria-hidden", "true");
  const footEmoji = document.createElement("span");
  footEmoji.className = "emoji";
  const footName = document.createElement("b");
  const footHint = document.createElement("span");
  footHint.className = "ep-foot-hint";
  footHint.textContent = "Enter to add · Esc to close";
  foot.append(footEmoji, footName, footHint);

  root.append(searchRow, tabs, body, foot);
  const built = { root, search, clear, tabs, body, footEmoji, footName };

  search.addEventListener("input", () => renderPicker(built));
  clear.addEventListener("click", () => {
    search.value = "";
    renderPicker(built);
    search.focus();
  });
  tabs.addEventListener("click", (event) => {
    const tab = (event.target as Element).closest<HTMLElement>(".ep-tab");
    const section = tab && body.querySelector<HTMLElement>(`[data-section="${tab.dataset.section}"]`);
    if (section) body.scrollTop = section.offsetTop;
  });
  body.addEventListener("scroll", () => markPickerTab(built));
  body.addEventListener("click", (event) => {
    const button = (event.target as Element).closest<HTMLElement>("[data-emoji]");
    if (button?.dataset.emoji) insertEmoji(button.dataset.emoji);
  });
  body.addEventListener("focusin", (event) => {
    const button = (event.target as Element).closest<HTMLButtonElement>("[data-emoji]");
    if (!button) return;
    body.querySelectorAll<HTMLButtonElement>("[data-emoji]").forEach((b) => (b.tabIndex = b === button ? 0 : -1));
    showPickerName(built, button);
  });
  body.addEventListener("mouseover", (event) => {
    const button = (event.target as Element).closest<HTMLElement>("[data-emoji]");
    if (button) showPickerName(built, button);
  });
  root.addEventListener("keydown", (event) => pickerKeys(built, event));
  return built;
}

function showPickerName(p: Picker, button: HTMLElement) {
  p.footEmoji.textContent = button.dataset.emoji ?? "";
  p.footName.textContent = EMOJI_NAMES.get(button.dataset.emoji ?? "") ?? "";
}

function emojiSection(id: string, label: string, list: readonly string[]): HTMLElement {
  const section = document.createElement("section");
  section.dataset.section = id;
  const heading = document.createElement("p");
  heading.className = "ep-label";
  heading.textContent = label;
  const grid = document.createElement("div");
  grid.className = "ep-grid";
  for (const emoji of list) {
    const button = pickerButton("", EMOJI_NAMES.get(emoji) ?? emoji);
    button.dataset.emoji = emoji;
    button.textContent = emoji;
    button.tabIndex = -1;
    grid.append(button);
  }
  section.append(heading, grid);
  return section;
}

// Every word of the search starts some word of the emoji's name or keywords.
function searchEmoji(query: string): string[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return [...EMOJI_SEARCH]
    .filter(([, parts]) => words.every((w) => parts.some((part) => part.startsWith(w))))
    .map(([emoji]) => emoji);
}

function renderPicker(p: Picker) {
  const query = p.search.value.trim();
  p.clear.hidden = query === "";
  p.tabs.hidden = query !== "";
  const sections: HTMLElement[] = [];
  if (query) {
    const found = searchEmoji(query);
    if (found.length) sections.push(emojiSection("results", "Results", found));
    else {
      const empty = document.createElement("div");
      empty.className = "ep-empty";
      const title = document.createElement("strong");
      title.textContent = `No emoji match "${query}"`;
      const hint = document.createElement("span");
      hint.textContent = "Try a word like smile, heart or rocket.";
      empty.append(title, hint);
      sections.push(empty);
    }
  } else {
    const recent = recentEmoji();
    // Recent and its tab stay hidden until something has been picked.
    p.tabs.querySelector<HTMLElement>('[data-section="recent"]')!.hidden = recent.length === 0;
    if (recent.length) sections.push(emojiSection("recent", "Recent", recent));
    for (const category of CATEGORIES) sections.push(emojiSection(category.id, category.label, category.list.map(([e]) => e)));
  }
  p.body.replaceChildren(...sections);
  p.body.scrollTop = 0;
  const first = p.body.querySelector<HTMLButtonElement>("[data-emoji]");
  if (first) {
    first.tabIndex = 0;
    showPickerName(p, first);
  } else {
    p.footEmoji.textContent = "";
    p.footName.textContent = "";
  }
  markPickerTab(p);
}

// The tab for the section at the top of the scroll is the pressed one.
function markPickerTab(p: Picker) {
  const sections = [...p.body.querySelectorAll<HTMLElement>("section[data-section]")];
  let current = sections[0]?.dataset.section ?? "";
  for (const section of sections) {
    if (section.offsetTop <= p.body.scrollTop + 16) current = section.dataset.section ?? "";
  }
  p.tabs.querySelectorAll<HTMLElement>(".ep-tab").forEach((tab) => tab.setAttribute("aria-pressed", String(tab.dataset.section === current)));
}

// Arrows move through the grid (up and down by a row), Home and End go to
// the ends of the section, ArrowDown leaves the search for the grid, and
// Enter in the search adds the first result. Enter or Space on an emoji
// clicks it.
function pickerKeys(p: Picker, event: KeyboardEvent) {
  if (event.isComposing) return;
  if (event.target === p.search) {
    const first = p.body.querySelector<HTMLButtonElement>("[data-emoji]");
    if (event.key === "Enter") {
      event.preventDefault();
      if (first?.dataset.emoji) insertEmoji(first.dataset.emoji);
    } else if (event.key === "ArrowDown" && first) {
      event.preventDefault();
      (p.body.querySelector<HTMLButtonElement>('[data-emoji][tabindex="0"]') ?? first).focus();
    }
    return;
  }
  const button = (event.target as Element).closest<HTMLButtonElement>("[data-emoji]");
  if (!button) return;
  const all = [...p.body.querySelectorAll<HTMLButtonElement>("[data-emoji]")];
  const inSection = [...(button.parentElement?.querySelectorAll<HTMLButtonElement>("[data-emoji]") ?? [])];
  const i = all.indexOf(button);
  const moves: Record<string, () => HTMLButtonElement | undefined> = {
    ArrowRight: () => all[i + 1],
    ArrowLeft: () => all[i - 1],
    ArrowDown: () => all[Math.min(i + GRID_COLUMNS, all.length - 1)],
    ArrowUp: () => (i < GRID_COLUMNS ? undefined : all[i - GRID_COLUMNS]),
    Home: () => inSection[0],
    End: () => inSection[inSection.length - 1],
  };
  const move = moves[event.key];
  if (!move) return;
  event.preventDefault();
  const next = move();
  if (next) {
    next.focus();
    next.scrollIntoView({ block: "nearest" });
  } else if (event.key === "ArrowUp") p.search.focus();
}

// Adds the emoji at the field's cursor. A field that has lost focus keeps
// its selection, so this works with the focus in the picker. The input event
// lets autogrow and the mention menu catch up.
function insertEmoji(emoji: string) {
  const field = pickerField;
  if (!field?.isConnected) return;
  const start = pickerDocked ? dockedCaret : field.selectionStart;
  const end = pickerDocked ? dockedCaretEnd : field.selectionEnd;
  if (field.maxLength > 0 && field.value.length - (end - start) + emoji.length > field.maxLength) return;
  field.setRangeText(emoji, start, end, "end");
  dockedCaret = dockedCaretEnd = start + emoji.length;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  rememberEmoji(emoji);
}

// Keeps a chat feed that was at its newest message there while the picker
// docks or undocks and the feed's height changes.
function keepFeedAtEnd(change: () => void) {
  const feed = document.getElementById("chat-feed");
  const atEnd = !!feed && feed.scrollHeight - feed.scrollTop - feed.clientHeight < 8;
  change();
  if (feed && atEnd) scrollToEnd(feed);
}

function closePicker(refocus: boolean) {
  const field = pickerField;
  if (pickerDocked && field) field.setSelectionRange(dockedCaret, dockedCaretEnd);
  keepFeedAtEnd(() => {
    picker?.root.remove();
    field?.form?.classList.remove("docked");
  });
  if (pickerToggle) {
    pickerToggle.setAttribute("aria-expanded", "false");
    pickerToggle.setAttribute("aria-label", "Emoji");
  }
  pickerField = null;
  pickerToggle = null;
  pickerDocked = false;
  // On a phone this runs inside the tap, so the keyboard comes back.
  if (refocus && field?.isConnected) field.focus();
}

function openPicker(toggle: HTMLButtonElement) {
  const field = document.getElementById(toggle.dataset.emojiFor ?? "");
  if (!(field instanceof HTMLTextAreaElement)) return;
  // The field it last served may have left the page with the picker in it
  // (the task sheet re-renders after a note is posted).
  if (picker && !picker.root.isConnected) {
    document.querySelectorAll("[data-emoji-for]").forEach((b) => b.setAttribute("aria-expanded", "false"));
    pickerField = null;
    pickerToggle = null;
  }
  if (pickerField) closePicker(false);
  picker ??= buildPicker();
  pickerField = field;
  pickerToggle = toggle;
  picker.search.value = "";
  renderPicker(picker);
  toggle.setAttribute("aria-expanded", "true");
  if (matchMedia("(pointer: coarse)").matches) return dockPicker(picker, field, toggle);
  picker.root.className = "ep ep-pop";
  // The row that holds the field: the chat composer, or the note form's row.
  const row = field.closest(".emoji-field")?.parentElement;
  if (!row) return;
  row.append(picker.root);
  // Open downward when there is no room above inside the scroll box (the task sheet).
  const box = row.closest(".sheet-body, .chat-screen")?.getBoundingClientRect().top ?? 0;
  picker.root.classList.toggle("ep-below", row.getBoundingClientRect().top - box < picker.root.offsetHeight + 8);
  // Sections have no position until the picker is on the page.
  markPickerTab(picker);
  picker.search.focus();
}

// Phones: the picker goes under the field's form and the keyboard goes down,
// so the picker stands in for it. In chat the feed shrinks to make room; in
// the task sheet the sheet scrolls to show it. The search box is not focused,
// since that would bring the keyboard back up.
function dockPicker(p: Picker, field: HTMLTextAreaElement, toggle: HTMLButtonElement) {
  const form = field.form;
  if (!form) return;
  pickerDocked = true;
  dockedCaret = field.selectionStart;
  dockedCaretEnd = field.selectionEnd;
  p.root.className = "ep ep-dock";
  toggle.setAttribute("aria-label", "Show keyboard");
  keepFeedAtEnd(() => {
    form.after(p.root);
    if (form.classList.contains("composer")) form.classList.add("docked");
  });
  field.blur();
  if (form.closest(".sheet-body")) p.root.scrollIntoView({ block: "end" });
  markPickerTab(p);
}

document.addEventListener("click", (event) => {
  const toggle = (event.target as Element | null)?.closest<HTMLButtonElement>("[data-emoji-for]");
  if (!toggle) return;
  if (pickerToggle === toggle && picker?.root.isConnected) closePicker(true);
  else openPicker(toggle);
});
// Desktop: a press anywhere outside the picker, its field and its button
// closes it. Phones: a press on the field closes it so the keyboard can come
// up; anywhere else (the feed, say) leaves it open.
document.addEventListener("pointerdown", (event) => {
  if (!pickerField || !picker) return;
  const target = event.target as Node;
  if (pickerDocked) {
    if (pickerField.contains(target)) closePicker(false);
    return;
  }
  if (picker.root.contains(target) || pickerField.contains(target) || pickerToggle?.contains(target)) return;
  closePicker(false);
});
document.addEventListener("focusin", (event) => {
  if (pickerDocked && event.target === pickerField) closePicker(false);
});
// Escape closes it and goes back to the field. Inside the task sheet's dialog
// the Escape is claimed, so the sheet stays open.
document.addEventListener(
  "keydown",
  (event) => {
    if (event.key !== "Escape" || event.isComposing || !pickerField || !picker?.root.isConnected) return;
    claimEscape(event);
    closePicker(true);
  },
  true,
);

// Welcome pop-up on Home: shown once, a few slides with Next and Back. Closing
// it any way (Skip, ×, Escape, Get started) records that it was seen.
const welcome = document.querySelector<HTMLDialogElement>("dialog[data-welcome]");
if (welcome) {
  const slides = Array.from(welcome.querySelectorAll<HTMLElement>("[data-slide]"));
  const dots = Array.from(welcome.querySelectorAll<HTMLElement>(".welcome-dots i"));
  const next = welcome.querySelector<HTMLButtonElement>("[data-welcome-next]")!;
  const back = welcome.querySelector<HTMLButtonElement>("[data-welcome-back]")!;
  const skip = welcome.querySelector<HTMLButtonElement>("[data-welcome-skip]")!;
  const status = welcome.querySelector<HTMLElement>("[data-welcome-status]")!;
  let at = 0;
  const show = (i: number) => {
    at = i;
    slides.forEach((s, n) => (s.hidden = n !== i));
    dots.forEach((d, n) => d.classList.toggle("on", n === i));
    const last = i === slides.length - 1;
    next.textContent = last ? "Get started" : "Next";
    back.hidden = i === 0;
    skip.hidden = last;
    status.textContent = `Step ${i + 1} of ${slides.length}`;
  };
  welcome.addEventListener("click", (event) => {
    const button = (event.target as Element).closest("button");
    if (!button) return;
    if (button === next && at < slides.length - 1) show(at + 1);
    else if (button === back) show(at - 1);
    else if (button === next || button.hasAttribute("data-welcome-done")) welcome.close();
  });
  welcome.addEventListener("close", () => htmx.ajax("POST", "/guide/welcomed", { swap: "none" }), { once: true });
  show(0);
  welcome.showModal();
}

// Light or dark: "system" follows the device; a choice is saved on this device.
// The head of the page applies it before drawing, so it never flashes.
type Theme = "system" | "light" | "dark";
function savedTheme(): Theme {
  try {
    const t = localStorage.getItem("theme");
    return t === "light" || t === "dark" ? t : "system";
  } catch {
    return "system";
  }
}
function applyTheme(theme: Theme) {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  try {
    if (theme === "system") localStorage.removeItem("theme");
    else localStorage.setItem("theme", theme);
  } catch {
    // Storage can be refused (private browsing); the choice still applies to this page.
  }
  document.querySelectorAll<HTMLElement>("[data-theme-choice]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.themeChoice === theme)));
}
applyTheme(savedTheme());
document.addEventListener("click", (event) => {
  const target = event.target as Element | null;
  const choice = target?.closest<HTMLElement>("[data-theme-choice]")?.dataset.themeChoice as Theme | undefined;
  if (choice) return applyTheme(choice);
  if (target?.closest("[data-theme-toggle]")) {
    const theme = savedTheme();
    const dark = theme === "dark" || (theme === "system" && matchMedia("(prefers-color-scheme: dark)").matches);
    applyTheme(dark ? "light" : "dark");
  }
});

// Sheets with panes (a file's options, rename, delete): <button data-show-pane="rename">
// shows that pane. Closing the sheet goes back to the first one.
document.addEventListener("click", (event) => {
  const button = (event.target as Element | null)?.closest<HTMLElement>("[data-show-pane]");
  const dialog = button?.closest("dialog");
  if (!button || !dialog) return;
  dialog.querySelectorAll<HTMLElement>("[data-pane]").forEach((pane) => (pane.hidden = pane.dataset.pane !== button.dataset.showPane));
  dialog.querySelector<HTMLInputElement>(`[data-pane="${button.dataset.showPane}"] input`)?.focus();
});
document.addEventListener(
  "close",
  (event) => {
    if (!(event.target instanceof HTMLDialogElement) || !event.target.hasAttribute("data-panes")) return;
    event.target.querySelectorAll<HTMLElement>("[data-pane]").forEach((pane) => (pane.hidden = pane.dataset.pane !== "menu"));
  },
  true,
);

// Documents: add a photo or a file to the open folder, one per request. Size
// and type are checked here before sending, and again on the server.
const MAX_UPLOAD = 25 * 1024 * 1024;
const IMAGE_EXTENSIONS = ["jpg", "jpeg", "png", "webp", "heic", "heif"];
const UPLOAD_EXTENSIONS = ["jpg", "jpeg", "png", "webp", "heic", "heif", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx"];
const megabytes = (bytes: number) => `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
const uploadForm = () => document.querySelector<HTMLFormElement>("form[data-upload]");

// An error row like the server's (documents/_upload_error.html), cloned from the page.
function uploadRow(name: string, message: string, retry = false): HTMLElement | null {
  const template = document.querySelector<HTMLTemplateElement>("#upload-error-template");
  const row = template?.content.firstElementChild?.cloneNode(true) as HTMLElement | undefined;
  if (!row) return null;
  const title = row.querySelector<HTMLElement>(".name")!;
  title.textContent = name;
  title.hidden = !name;
  row.querySelector(".err-text")!.textContent = message;
  if (retry) {
    const again = document.createElement("button");
    again.type = "button";
    again.className = "text-link mt-1.5 min-h-8";
    again.dataset.retryUpload = "";
    again.textContent = "Try again";
    row.querySelector(".err-text")!.after(again);
  }
  return row;
}

function progressRow(name: string): HTMLElement {
  const row = document.createElement("div");
  row.className = "list-row";
  row.dataset.uploadProgress = "";
  const tile = document.createElement("span");
  tile.className = "tile";
  const ext = (name.split(".").pop() ?? "").toLowerCase();
  const icon = document.querySelector<HTMLTemplateElement>("#image-icon-template")?.content.firstElementChild;
  if (IMAGE_EXTENSIONS.includes(ext) && icon) {
    tile.append(icon.cloneNode(true));
  } else {
    const label = document.createElement("span");
    label.className = "tile-type";
    label.textContent = ext.toUpperCase();
    tile.append(label);
  }
  const body = document.createElement("div");
  body.className = "min-w-0 flex-1";
  const title = document.createElement("p");
  title.className = "name truncate-1 m-0";
  title.textContent = name;
  const bar = document.createElement("div");
  bar.className = "bar mt-1.5";
  bar.innerHTML = '<span style="width:0%"></span>';
  const text = document.createElement("p");
  text.className = "meta mt-1 mb-0";
  text.dataset.progressText = "";
  text.textContent = "Starting";
  body.append(title, bar, text);
  row.append(tile, body);
  return row;
}

// A camera photo arrives as "image.jpg"; give it the date instead.
function namedPhoto(file: File): File {
  const match = /^image\.(jpe?g|png|heic|heif)$/i.exec(file.name);
  if (!match) return file;
  const now = new Date();
  const day = now.toLocaleDateString("en-GB", { day: "numeric", month: "short" });
  const time = now.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
  return new File([file], `Photo ${day}, ${time}.${match[1].toLowerCase()}`, { type: file.type });
}

// Files go one at a time: the next starts when the current request finishes,
// whether it worked or not. A failed upload's file is kept on its row for Try again.
type Upload = { file: File; row: HTMLElement };
const uploadQueue: Upload[] = [];
let currentUpload: Upload | null = null;
const failedFiles = new WeakMap<Element, File>();

function queueUpload(file: File) {
  const row = progressRow(file.name);
  row.querySelector("[data-progress-text]")!.textContent = "Waiting";
  // A waiting file can be taken off the queue with the same X as an error row.
  const cancel = document.querySelector<HTMLTemplateElement>("#upload-error-template")?.content.querySelector("[data-dismiss]")?.cloneNode(true) as HTMLElement | undefined;
  if (cancel) {
    delete cancel.dataset.dismiss;
    cancel.dataset.cancelUpload = "";
    cancel.setAttribute("aria-label", `Don't add ${file.name}`);
    row.append(cancel);
  }
  document.getElementById("uploads")?.append(row);
  uploadQueue.push({ file, row });
  void startNextUpload();
}

async function startNextUpload() {
  const form = uploadForm();
  const input = form?.querySelector<HTMLInputElement>("input[type=file]");
  if (currentUpload || !form || !input || document.querySelector("[data-stale-banner]")) return;
  const next = uploadQueue.shift();
  if (!next) return;
  try {
    const files = new DataTransfer();
    files.items.add(next.file);
    input.files = files.files;
  } catch {
    // This browser can't hand the file to the form; say so and carry on with the rest.
    next.row.replaceWith(uploadRow(next.file.name, "Couldn't add it. Use Add instead.") ?? "");
    return void startNextUpload();
  }
  currentUpload = next;
  // Ask first, so a stale tab is caught before it sends a large body. The
  // file then waits at the front of the queue, behind the banner. HX-Request,
  // because the server only checks the claim on HTMX GETs.
  const stale = await fetch("/orgs/current", { headers: { "HX-Request": "true", ...orgHeaders() }, credentials: "same-origin" }).then(staleRefusal, () => false);
  if (stale) {
    currentUpload = null;
    uploadQueue.unshift(next);
    return;
  }
  next.row.querySelector("[data-cancel-upload]")?.remove();
  next.row.querySelector("[data-progress-text]")!.textContent = "Starting";
  form.requestSubmit();
}

// Checks one file before it joins the queue; a bad one gets an error row instead.
function addUpload(file: File) {
  const ext = file.name.split(".").pop()?.toLowerCase() ?? "";
  const problem =
    file.size > MAX_UPLOAD
      ? `Too big: ${megabytes(file.size)}. The limit is 25 MB. Try saving a smaller copy.`
      : UPLOAD_EXTENSIONS.includes(ext)
        ? ""
        : "This kind of file can't be added. Use a photo, PDF or Office file.";
  if (problem) {
    const row = uploadRow(file.name, problem);
    if (row) document.getElementById("uploads")?.prepend(row);
    return;
  }
  queueUpload(namedPhoto(file));
}

document.addEventListener("click", (event) => {
  const target = event.target as Element | null;
  const picker = target?.closest<HTMLElement>("[data-pick]");
  const input = uploadForm()?.querySelector<HTMLInputElement>("input[type=file]");
  if (picker && input) {
    picker.closest("dialog")?.close();
    const photo = picker.dataset.pick === "photo";
    input.accept = photo ? "image/*" : ".pdf,.doc,.docx,.xls,.xlsx,.ppt,.pptx,image/*";
    input.multiple = !photo;
    input.click();
    return;
  }
  const row = target?.closest<HTMLElement>("[data-upload-row]");
  const file = row && failedFiles.get(row);
  if (target?.closest("[data-retry-upload]") && row && file) {
    row.remove();
    queueUpload(file);
    return;
  }
  if (target?.closest("[data-dismiss]")) row?.remove();
  const waiting = target?.closest("[data-cancel-upload]")?.closest<HTMLElement>("[data-upload-progress]");
  const queued = uploadQueue.findIndex((upload) => upload.row === waiting);
  if (waiting && queued >= 0) {
    uploadQueue.splice(queued, 1);
    waiting.remove();
  }
});

window.addEventListener("beforeunload", (event) => {
  if (currentUpload || uploadQueue.length) event.preventDefault();
});

document.addEventListener("change", (event) => {
  const input = event.target;
  if (!(input instanceof HTMLInputElement) || !input.form?.hasAttribute("data-upload")) return;
  const files = Array.from(input.files ?? []);
  input.value = "";
  files.forEach(addUpload);
});

// Dragging files from the computer onto an open folder's pane. Any file drag is
// cancelled on every page, so a drop never opens the file in the browser; only
// the open folder's pane takes it. dragleave fires for every child crossed, so a
// counter tells when the drag has left the window.
let dragDepth = 0;
const fileDrag = (event: DragEvent) => !!event.dataTransfer?.types.includes("Files");
const overDropPane = (event: DragEvent) => !!uploadForm() && !!(event.target as Element | null)?.closest?.(".drop-pane");

function showDropOverlay(show: boolean) {
  if (!show) dragDepth = 0;
  const overlay = document.querySelector<HTMLElement>("[data-drop-overlay]");
  if (overlay && overlay.hidden === show) overlay.hidden = !show;
}

document.addEventListener("dragenter", (event) => {
  if (!fileDrag(event)) return;
  event.preventDefault();
  dragDepth += 1;
});
document.addEventListener("dragover", (event) => {
  if (!fileDrag(event)) return;
  event.preventDefault();
  const accept = overDropPane(event);
  event.dataTransfer!.dropEffect = accept ? "copy" : "none";
  showDropOverlay(accept);
});
document.addEventListener("dragleave", (event) => {
  if (!fileDrag(event)) return;
  dragDepth -= 1;
  if (dragDepth <= 0) showDropOverlay(false);
});
document.addEventListener("drop", (event) => {
  if (!fileDrag(event)) return;
  event.preventDefault();
  showDropOverlay(false);
  if (overDropPane(event)) Array.from(event.dataTransfer!.files).forEach(addUpload);
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") showDropOverlay(false);
});
// pointermove never fires during a drag, so one after it means a dragleave was missed.
document.addEventListener("pointermove", () => showDropOverlay(false));

// Progress fires for the upload and then for the download of the response;
// only the upload part moves the bar.
document.body.addEventListener("htmx:xhr:progress", (event) => {
  const { loaded, total, lengthComputable } = (event as CustomEvent<{ loaded: number; total: number; lengthComputable: boolean }>).detail;
  const row = currentUpload?.row;
  if (!(event.target as Element).closest?.("form[data-upload]") || !row || row.dataset.sent || !lengthComputable) return;
  row.querySelector<HTMLElement>(".bar > span")!.style.width = `${Math.round((loaded / total) * 100)}%`;
  const text = row.querySelector<HTMLElement>("[data-progress-text]")!;
  text.textContent = `${megabytes(loaded)} of ${megabytes(total)}`;
  if (loaded >= total) {
    row.dataset.sent = "1";
    text.textContent = "Saving";
  }
});

document.body.addEventListener("htmx:afterRequest", (event) => {
  const { elt, successful, xhr } = (event as CustomEvent<{ elt: Element; successful: boolean; xhr: XMLHttpRequest }>).detail;
  if (!(elt instanceof HTMLFormElement) || !elt.hasAttribute("data-upload") || !currentUpload) return;
  const { file, row } = currentUpload;
  currentUpload = null;
  elt.reset();
  // Refused as a stale tab: the banner is up, and the file waits for "Back".
  if (xhr.status === 409) {
    delete row.dataset.sent;
    row.querySelector<HTMLElement>(".bar > span")!.style.width = "0%";
    row.querySelector("[data-progress-text]")!.textContent = "Waiting";
    uploadQueue.unshift({ file, row });
    return;
  }
  row.remove();
  // htmx still holds this form's request until the handler returns, so start the next one after.
  setTimeout(startNextUpload);
  // A refusal comes back as its own error row.
  if (successful || xhr.status === 422) return;
  // Status 0: the connection dropped. Keep the file so Try again can resend it.
  const dropped = xhr.status === 0;
  const message = dropped
    ? "Stopped part way. Check your connection."
    : xhr.status === 413
      ? "Too big. The limit is 25 MB. Try saving a smaller copy."
      : "Couldn't add it. Try again, or reload the page.";
  const failed = uploadRow(file.name, message, dropped);
  if (!failed) return;
  if (dropped) failedFiles.set(failed, file);
  document.getElementById("uploads")?.prepend(failed);
});

// <button data-copy-text="..."> copies its own text, such as a login's username.
document.addEventListener("click", (event) => {
  const button = (event.target as Element | null)?.closest<HTMLElement>("[data-copy-text]");
  if (!button) return;
  void navigator.clipboard.writeText(button.dataset.copyText ?? "").then(
    () => showToast({ kind: "success", message: button.dataset.copyMessage ?? "Copied" }),
    () => showToast({ kind: "error", message: "Couldn't copy. Select it and copy it instead." }),
  );
});

// Password fields with an eye button: show or hide what you typed.
document.addEventListener("click", (event) => {
  const button = (event.target as Element | null)?.closest<HTMLElement>("[data-toggle-password]");
  const input = button?.parentElement?.querySelector<HTMLInputElement>("input");
  if (!button || !input) return;
  const showing = input.type === "text";
  input.type = showing ? "password" : "text";
  button.setAttribute("aria-pressed", String(!showing));
});

// Logins: Show and Copy need the app password once every 5 minutes. A shown
// secret hides itself after 30 seconds, and is removed from the page when it
// hides, when its sheet closes, and when the page is left.
type SecretAsk = { kind: "show" | "copy"; loginId: string; field: string; box: HTMLElement };
const HIDE_AFTER_SECONDS = 30;
let pendingSecret: SecretAsk | null = null;
const countdowns = new Map<HTMLElement, number>();

function postForm(url: string, values: Record<string, string>): Promise<Response> {
  return fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded", ...orgHeaders() },
    body: new URLSearchParams(values),
    credentials: "same-origin",
  });
}

function hideSecret(box: HTMLElement) {
  window.clearInterval(countdowns.get(box));
  countdowns.delete(box);
  box.querySelector("[data-secret-slot]")?.replaceChildren();
  const hidden = box.querySelector<HTMLElement>("[data-secret-hidden]");
  if (hidden) hidden.hidden = false;
}

function hideAllSecrets(root: ParentNode = document) {
  root.querySelectorAll<HTMLElement>("[data-secret]").forEach(hideSecret);
}

function secretAsk(button: Element, kind: SecretAsk["kind"]): SecretAsk | null {
  const box = button.closest<HTMLElement>("[data-secret]");
  const loginId = button.closest<HTMLElement>("[data-login]")?.dataset.login;
  if (!box || !loginId) return null;
  return { kind, loginId, field: box.dataset.secret ?? "password", box };
}

// "Unlocked for 4 more minutes" counts down and hides when the unlock ends,
// so it never claims an unlock the server has already dropped.
function tickUnlocked(line: HTMLElement) {
  const left = Number(line.dataset.seconds ?? "0") - (Date.now() - Number(line.dataset.since ?? Date.now())) / 1000;
  line.hidden = left <= 0;
  const minutes = Math.ceil(left / 60);
  const text = line.querySelector("[data-unlocked-text]");
  if (text) text.textContent = `Unlocked for ${minutes} more ${minutes === 1 ? "minute" : "minutes"}`;
}
function startUnlocked(root: ParentNode, seconds?: number) {
  root.querySelectorAll<HTMLElement>("[data-unlocked-line]").forEach((line) => {
    if (seconds !== undefined) line.dataset.seconds = String(seconds);
    line.dataset.since = String(Date.now());
    tickUnlocked(line);
  });
}
window.setInterval(() => document.querySelectorAll<HTMLElement>("[data-unlocked-line]").forEach(tickUnlocked), 5000);
startUnlocked(document);
document.body.addEventListener("htmx:afterSwap", (event) => startUnlocked((event as CustomEvent<{ target: Element }>).detail.target));

function markUnlocked() {
  startUnlocked(document, 5 * 60);
}

// The edit sheet opens straight away when a page asks for it (?edit=1).
document.querySelector<HTMLDialogElement>("dialog[data-open-on-load]")?.showModal();

// A refused request: signed out (the session ended) or an error message.
async function secretRefused(res: Response) {
  if (res.redirected || res.status === 401) {
    window.location.href = "/login";
    return;
  }
  showToast({ kind: "error", message: res.status === 403 ? "You can't see this password." : "Couldn't show it. Try again." });
}

function askForPassword(ask: SecretAsk) {
  const dialog = document.querySelector<HTMLDialogElement>("dialog[data-unlock]");
  if (!dialog) return;
  pendingSecret = ask;
  dialog.querySelectorAll<HTMLElement>("[data-unlock-pane]").forEach((pane) => (pane.hidden = pane.dataset.unlockPane !== "form"));
  const error = dialog.querySelector<HTMLElement>("[data-unlock-error]");
  if (error) error.hidden = true;
  dialog.querySelector("input[name=password]")?.removeAttribute("aria-invalid");
  const what = ask.field === "recovery" ? "recovery codes" : "password";
  dialog.querySelector("[data-unlock-submit]")!.textContent = `${ask.kind === "copy" ? "Copy" : "Show"} ${what}`;
  dialog.showModal();
  dialog.querySelector<HTMLInputElement>("input[name=password]")?.focus();
}

async function showSecret(ask: SecretAsk) {
  const res = await postForm(`/documents/logins/${ask.loginId}/reveal`, { field: ask.field });
  if (staleRefusal(res)) return;
  if (res.status === 423) return askForPassword(ask);
  if (!res.ok || res.redirected) return secretRefused(res);
  const slot = ask.box.querySelector<HTMLElement>("[data-secret-slot]");
  if (!slot) return;
  hideSecret(ask.box);
  // The server escapes the secret; this is its own fragment.
  slot.innerHTML = await res.text();
  const hidden = ask.box.querySelector<HTMLElement>("[data-secret-hidden]");
  if (hidden) hidden.hidden = true;
  const bar = slot.querySelector<HTMLElement>("[data-countdown-bar]");
  const text = slot.querySelector<HTMLElement>("[data-countdown-text]");
  let left = HIDE_AFTER_SECONDS;
  countdowns.set(
    ask.box,
    window.setInterval(() => {
      left -= 1;
      if (left <= 0) return hideSecret(ask.box);
      if (bar) bar.style.width = `${(left / HIDE_AFTER_SECONDS) * 100}%`;
      if (text) text.textContent = `Hides in ${left}s`;
    }, 1000),
  );
}

class NeedsPassword extends Error {}

// Copies without showing. The clipboard write starts inside the tap, with the
// text still on its way: iOS Safari only allows that through ClipboardItem.
function copySecret(ask: SecretAsk, unlocking?: Promise<Response>) {
  const what = ask.field === "recovery" ? "Recovery codes" : "Password";
  const text = (async () => {
    const unlocked = unlocking && (await unlocking);
    // A stale refusal is dealt with by the unlock's own handler below.
    if (unlocked && unlocked.status === 409) throw new Error("refused");
    if (unlocked && !unlocked.ok) throw new NeedsPassword();
    const res = await postForm(`/documents/logins/${ask.loginId}/copy`, { field: ask.field });
    if (staleRefusal(res)) throw new Error("refused");
    if (res.status === 423) throw new NeedsPassword();
    if (!res.ok || res.redirected) {
      await secretRefused(res);
      throw new Error("refused");
    }
    return res.text();
  })();
  const written =
    typeof ClipboardItem !== "undefined" && navigator.clipboard?.write
      ? navigator.clipboard.write([new ClipboardItem({ "text/plain": text.then((t) => new Blob([t], { type: "text/plain" })) })])
      : text.then((t) => navigator.clipboard.writeText(t));
  written.then(
    () => showToast({ kind: "success", message: `${what} copied` }),
    (err) => {
      if (err instanceof NeedsPassword) {
        if (!unlocking) askForPassword(ask);
      } else if (!(err instanceof Error && err.message === "refused")) {
        showToast({ kind: "error", message: "Couldn't copy. Show it instead, then copy it." });
      }
    },
  );
  // Clipboard managers can keep it; at least don't keep a promise of it around.
  void text.catch(() => undefined);
}

document.addEventListener("click", (event) => {
  const target = event.target as Element | null;
  const show = target?.closest("[data-secret-show]");
  const copy = target?.closest("[data-secret-copy]");
  if (show || copy) {
    const ask = secretAsk((show ?? copy)!, show ? "show" : "copy");
    if (!ask) return;
    if (ask.kind === "show") void showSecret(ask);
    else copySecret(ask);
    return;
  }
  const shown = target?.closest("[data-secret-copy-shown]");
  if (shown) {
    const box = shown.closest<HTMLElement>("[data-secret]");
    const value = box?.querySelector("[data-secret-value]")?.textContent ?? "";
    const what = box?.dataset.secret === "recovery" ? "Recovery codes" : "Password";
    void navigator.clipboard.writeText(value).then(
      () => showToast({ kind: "success", message: `${what} copied` }),
      () => showToast({ kind: "error", message: "Couldn't copy. Select it and copy it instead." }),
    );
    return;
  }
  const hide = target?.closest("[data-secret-hide]")?.closest<HTMLElement>("[data-secret]");
  if (hide) hideSecret(hide);
});

document.addEventListener("submit", (event) => {
  const form = event.target;
  if (!(form instanceof HTMLFormElement) || !form.hasAttribute("data-unlock-form")) return;
  event.preventDefault();
  const dialog = form.closest("dialog");
  const input = form.querySelector<HTMLInputElement>("input[name=password]");
  const error = form.querySelector<HTMLElement>("[data-unlock-error]");
  const ask = pendingSecret;
  if (!dialog || !input || !ask) return;
  const unlocking = postForm("/documents/logins/unlock", { password: input.value, login_id: ask.loginId });
  input.value = "";
  input.removeAttribute("aria-invalid");
  if (error) error.hidden = true;
  // Copy has to begin inside this tap, so it waits on the unlock itself.
  if (ask.kind === "copy") copySecret(ask, unlocking);
  void unlocking.then(async (res) => {
    if (staleRefusal(res)) return dialog.close();
    if (res.ok && !res.redirected) {
      pendingSecret = null;
      dialog.close();
      markUnlocked();
      if (ask.kind === "show") await showSecret(ask);
      return;
    }
    if (res.status === 401) {
      dialog.querySelectorAll<HTMLElement>("[data-unlock-pane]").forEach((pane) => (pane.hidden = pane.dataset.unlockPane !== "locked"));
      return;
    }
    if (res.redirected) {
      window.location.href = "/login";
      return;
    }
    if (error) {
      error.textContent = (await res.text()) || "Couldn't check your password. Try again.";
      error.hidden = false;
    }
    input.setAttribute("aria-invalid", "true");
    input.focus();
  });
});

// Desktop opens a login in the side sheet, so the folder stays behind it.
document.addEventListener("click", (event) => {
  const link = (event.target as Element | null)?.closest<HTMLAnchorElement>("a[data-login-sheet]");
  const sheet = document.querySelector<HTMLDialogElement>("#login-sheet");
  if (!link || !sheet || !matchMedia("(min-width: 900px)").matches) return;
  event.preventDefault();
  void htmx.ajax("GET", link.href, { target: "#login-sheet-body", swap: "innerHTML" }).then(() => sheet.showModal());
});
document.addEventListener(
  "close",
  (event) => {
    const dialog = event.target;
    if (!(dialog instanceof HTMLDialogElement)) return;
    hideAllSecrets(dialog);
    if (dialog.id === "login-sheet") dialog.querySelector("#login-sheet-body")?.replaceChildren();
    if (dialog.hasAttribute("data-unlock")) {
      const input = dialog.querySelector<HTMLInputElement>("input[name=password]");
      if (input) input.value = "";
    }
  },
  true,
);
window.addEventListener("pagehide", () => hideAllSecrets());

// The organisation switcher (docs/designs/org-switcher.html): a menu under the
// sidebar's org name, and on phones a sheet from Home's header or More. Both
// load their rows from /orgs the first time they open. Without JavaScript the
// triggers are links to the /orgs page.
const orgMenuButton = document.querySelector<HTMLElement>("[data-org-menu]");
const orgMenu = document.getElementById("org-menu");
const orgSheet = document.querySelector<HTMLDialogElement>("#org-sheet");
const orgSheetBody = document.getElementById("org-sheet-body");

const orgItems = (list: Element) => Array.from(list.querySelectorAll<HTMLElement>("[role^=menuitem]"));

// Rows are reached with the arrow keys, not Tab, inside the menu and the sheet.
async function loadOrgs(list: HTMLElement, force = false) {
  if (force || !list.querySelector("[data-org-list]")) await htmx.ajax("GET", "/orgs", { target: `#${list.id}`, swap: "innerHTML" });
  orgItems(list).forEach((item) => (item.tabIndex = -1));
}

function focusOrg(list: HTMLElement, which: "current" | "last") {
  const items = orgItems(list);
  const target = which === "last" ? items[items.length - 1] : (items.find((el) => el.getAttribute("aria-checked") === "true") ?? items[0]);
  target?.focus();
}

const orgMenuOpen = () => orgMenuButton?.getAttribute("aria-expanded") === "true";

async function openOrgMenu(which: "current" | "last" = "current") {
  if (!orgMenuButton || !orgMenu) return;
  orgMenuButton.setAttribute("aria-expanded", "true");
  orgMenu.hidden = false;
  await loadOrgs(orgMenu);
  if (orgMenuOpen()) focusOrg(orgMenu, which);
}

function closeOrgMenu(refocus = true) {
  if (!orgMenuButton || !orgMenu || !orgMenuOpen()) return;
  orgMenuButton.setAttribute("aria-expanded", "false");
  orgMenu.hidden = true;
  if (refocus) orgMenuButton.focus();
}

async function openOrgSheet() {
  if (!orgSheet || !orgSheetBody) return;
  orgSheet.showModal();
  await loadOrgs(orgSheetBody);
  focusOrg(orgSheetBody, "current");
}

// Enter or Space on a row: switch, nothing on a pending one, and the current
// org just closes the list.
function pickOrg(item: HTMLElement | undefined) {
  if (!item || item.getAttribute("aria-disabled") === "true") return;
  if (item.hasAttribute("data-org-current")) {
    closeOrgMenu();
    orgSheet?.close();
    return;
  }
  item.click();
}

function orgListKeys(list: HTMLElement, event: KeyboardEvent) {
  if (event.isComposing) return;
  const items = orgItems(list);
  const i = items.indexOf(document.activeElement as HTMLElement);
  const go = (n: number) => {
    event.preventDefault();
    items[(n + items.length) % items.length]?.focus();
  };
  if (event.key === "ArrowDown") go(i + 1);
  else if (event.key === "ArrowUp") go(i - 1);
  else if (event.key === "Home") go(0);
  else if (event.key === "End") go(items.length - 1);
  else if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    pickOrg(items[i]);
  } else if (list === orgMenu && event.key === "Escape") {
    event.preventDefault();
    closeOrgMenu();
  } else if (list === orgMenu && event.key === "Tab") closeOrgMenu(false);
  else if (event.key.length === 1 && /\S/.test(event.key)) {
    // The next org whose name starts with that letter.
    const key = event.key.toLowerCase();
    for (let step = 1; step <= items.length; step++) {
      const item = items[(i + step) % items.length];
      if (item.querySelector(".org-name")?.textContent?.trim().toLowerCase().startsWith(key)) {
        item.focus();
        break;
      }
    }
  }
}

if (orgMenuButton && orgMenu) {
  orgMenuButton.addEventListener("click", (event) => {
    event.preventDefault();
    if (orgMenuOpen()) closeOrgMenu();
    else void openOrgMenu();
  });
  orgMenuButton.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      void openOrgMenu();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      void openOrgMenu("last");
    }
  });
  orgMenu.addEventListener("keydown", (event) => orgListKeys(orgMenu, event));
  document.addEventListener("click", (event) => {
    const target = event.target as Node;
    if (orgMenuOpen() && !orgMenu.contains(target) && !orgMenuButton.contains(target)) closeOrgMenu(false);
  });
}
orgSheetBody?.addEventListener("keydown", (event) => orgListKeys(orgSheetBody, event));
document.addEventListener("click", (event) => {
  const opener = (event.target as Element | null)?.closest("[data-org-sheet]");
  if (!opener || !orgSheet) return;
  event.preventDefault();
  void openOrgSheet();
});
document.addEventListener("click", (event) => {
  const current = (event.target as Element | null)?.closest("[data-org-current]");
  if (!current || !current.closest("#org-menu, #org-sheet")) return;
  closeOrgMenu();
  orgSheet?.close();
});

// While a switch runs: a spinner and "Switching…" on its row, the others dimmed.
function switchingState(row: HTMLElement, on: boolean) {
  const list = row.closest<HTMLElement>("[data-org-list]");
  list?.querySelectorAll<HTMLElement>(".org-row").forEach((other) => other !== row && other.classList.toggle("dim", on));
  row.classList.toggle("loading", on);
  const meta = row.querySelector<HTMLElement>("[data-org-role]");
  if (meta) {
    meta.dataset.role ??= meta.textContent ?? "";
    meta.textContent = on ? "Switching…" : meta.dataset.role;
  }
}
document.body.addEventListener("htmx:beforeRequest", (event) => {
  const elt = (event as CustomEvent<{ elt: Element }>).detail.elt;
  if (elt instanceof HTMLElement && elt.matches(".org-row[data-org-name]")) switchingState(elt, true);
});
document.body.addEventListener("htmx:afterRequest", (event) => {
  const { elt, successful, xhr } = (event as CustomEvent<{ elt: Element; successful: boolean; xhr: XMLHttpRequest }>).detail;
  if (!(elt instanceof HTMLElement) || !elt.matches(".org-row[data-org-name]") || successful) return;
  const name = elt.dataset.orgName ?? "";
  switchingState(elt, false);
  if (xhr.status === 404) {
    showToast({ kind: "error", message: `You can't open ${name} any more.` });
    const list = elt.closest<HTMLElement>("#org-menu, #org-sheet-body");
    if (list) void loadOrgs(list, true).then(() => focusOrg(list, "current"));
  } else {
    showToast({ kind: "error", message: `Couldn't switch to ${name}. Check your connection and try again.` });
  }
});
