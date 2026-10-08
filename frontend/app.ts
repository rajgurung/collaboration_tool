// Small client helpers. HTMX does the heavy lifting; this covers dialogs and toasts.

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

// Chat: Enter sends, Shift+Enter adds a new line.
document.addEventListener("keydown", (event) => {
  const target = event.target;
  if (!(target instanceof HTMLTextAreaElement) || !target.hasAttribute("data-enter-submits")) return;
  if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
  event.preventDefault();
  if (target.value.trim()) target.form?.requestSubmit();
});

// Keep feeds marked data-autoscroll pinned to the newest message.
function scrollToEnd(el: Element) {
  el.scrollTop = el.scrollHeight;
}
document.querySelectorAll("[data-autoscroll]").forEach(scrollToEnd);
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

const reconnecting = new WeakSet<Element>();
document.body.addEventListener("htmx:wsClose", (event) => {
  reconnecting.add((event as CustomEvent<{ elt: Element }>).detail.elt);
});
document.body.addEventListener("htmx:wsOpen", (event) => {
  const elt = (event as CustomEvent<{ elt: Element }>).detail.elt;
  if (!reconnecting.has(elt)) return;
  reconnecting.delete(elt);
  const url = elt.getAttribute("data-chat-feed-url");
  if (url) void htmx.ajax("GET", url, { target: "#chat-feed", swap: "innerHTML" });
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

// Greeting and date in the viewer's own time zone (the server runs in UTC).
document.querySelectorAll<HTMLElement>("[data-greeting]").forEach((el) => {
  const hour = new Date().getHours();
  const part = hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening";
  el.textContent = `${part}, ${el.dataset.greeting}`;
});
document.querySelectorAll<HTMLElement>("[data-local-date]").forEach((el) => {
  el.textContent = new Date().toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
});

// <button data-toggle="#id"> shows or hides an element and focuses its first input.
document.addEventListener("click", (event) => {
  const button = (event.target as Element | null)?.closest<HTMLElement>("[data-toggle]");
  if (!button) return;
  const target = document.querySelector<HTMLElement>(button.dataset.toggle ?? "");
  if (!target) return;
  target.classList.toggle("hidden");
  if (!target.classList.contains("hidden")) target.querySelector<HTMLInputElement>("input:not([type=hidden])")?.focus();
});

// <button data-copy="#input"> copies that input's value.
document.addEventListener("click", (event) => {
  const button = (event.target as Element | null)?.closest<HTMLElement>("[data-copy]");
  if (!button) return;
  const input = document.querySelector<HTMLInputElement>(button.dataset.copy ?? "");
  if (!input) return;
  void navigator.clipboard.writeText(input.value).then(
    () => showToast({ kind: "success", message: "Link copied" }),
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
