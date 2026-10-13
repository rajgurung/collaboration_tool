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

// Chat message actions: a tap, click, Enter or Space on a bubble opens its
// sheet (react, who reacted). Taps on a chip, receipt, link or button do their
// own thing, and a tap that ends a text selection leaves the selection alone.
function openActions(bubble: HTMLElement) {
  const url = bubble.dataset.actionsUrl;
  const dialog = document.querySelector<HTMLDialogElement>("#chat-dialog");
  const body = dialog?.querySelector<HTMLElement>("#chat-dialog-body");
  if (!url || !dialog || !body) return;
  const failed = () => showToast({ kind: "error", message: "Couldn't open that message. Check your connection and try again." });
  fetch(url, { headers: { "HX-Request": "true" }, credentials: "same-origin" })
    .then(async (res) => {
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
  if (!bubble || target?.closest("a, button, .react, .thread-link, .msg-receipt")) return;
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
  const holder = el?.closest<HTMLElement>(".reacts, .msg-tools");
  return holder && el instanceof HTMLElement && el.dataset.emoji ? { holder: holder.id, emoji: el.dataset.emoji } : null;
}
// Only when the swap dropped focus to the page, never taking it from elsewhere.
function restoreFocus(was: FocusedReaction) {
  if (!was || document.activeElement !== document.body) return;
  const holder = document.getElementById(was.holder);
  const button = Array.from(holder?.querySelectorAll<HTMLElement>("[data-emoji]") ?? []).find((b) => b.dataset.emoji === was.emoji);
  if (!holder || !button) return;
  // The new toolbar only shows once something in its message has focus, so show it to focus it.
  holder.style.display = "flex";
  button.focus();
  holder.style.removeProperty("display");
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
    headers: { "HX-Request": "true" },
    body: new URLSearchParams({ emoji, on: on ? "1" : "0" }),
    credentials: "same-origin",
  })
    .then(async (res) => {
      if (!res.ok || res.redirected) throw new Error(`status ${res.status}`);
      swapById(await res.text());
    })
    .catch(() => {
      showMine(message, emoji, url, !on);
      showToast({ kind: "error", message: `Couldn't ${on ? "add" : "remove"} your reaction. Check your connection and try again.` });
    })
    .finally(() => reacting.delete(key));
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
  startNextUpload();
}

function startNextUpload() {
  const form = uploadForm();
  const input = form?.querySelector<HTMLInputElement>("input[type=file]");
  if (currentUpload || !form || !input) return;
  const next = uploadQueue.shift();
  if (!next) return;
  try {
    const files = new DataTransfer();
    files.items.add(next.file);
    input.files = files.files;
  } catch {
    // This browser can't hand the file to the form; say so and carry on with the rest.
    next.row.replaceWith(uploadRow(next.file.name, "Couldn't add it. Use Add instead.") ?? "");
    return startNextUpload();
  }
  currentUpload = next;
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
  row.remove();
  elt.reset();
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
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
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
    if (unlocking && !(await unlocking).ok) throw new NeedsPassword();
    const res = await postForm(`/documents/logins/${ask.loginId}/copy`, { field: ask.field });
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
