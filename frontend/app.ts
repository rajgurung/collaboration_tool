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

// Notifications: the server pushes a `notify` event when this person gets a new
// one, and everything listening for "notifications-changed" reloads. The
// browser reconnects on its own; badges also poll every 60s as a fallback.
if (document.querySelector('[hx-get^="/notifications/unread"]')) {
  const events = new EventSource("/notifications/stream");
  events.addEventListener("notify", () => htmx.trigger(document.body, "notifications-changed"));
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
      li.textContent = `@${name}`;
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
