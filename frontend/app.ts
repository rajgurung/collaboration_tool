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
