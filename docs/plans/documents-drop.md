# Documents: drag and drop to upload

Status: Approved by Raj, 12 October 2026 · Small change (plan, build, review, PR)

## Summary

On desktop, drag one or more files from the computer onto an open folder and
they upload into it. Files go one at a time, through the same upload route and
the same checks as Add. No server change.

## What changes

1. **Drop target.** When a folder is open (`form[data-upload]` exists), the
   folder page accepts drops (`dragenter`, `dragover`, `dragleave`, `drop` on
   `document`, only for drags that carry files: `dataTransfer.types` includes
   `Files`). Drops anywhere else, or on pages without an open folder, do
   nothing and never navigate the browser to the file (always
   `preventDefault` on file drags while a folder is open).
2. **Overlay.** While a file drag is over the page, show a full-panel overlay
   over the folder pane: dashed border, upload icon, "Drop to add to
   {folder name}", and "Photos, PDF and Office files, up to 25 MB each".
   Uses existing tokens (`--accent`, `--active-bg`, `--surface`); readable in
   light and dark. Hidden on drop, on leaving the window, and on Escape.
   Counter-based `dragenter`/`dragleave` so child elements don't flicker it.
3. **Queue.** Each dropped file is checked with the existing size and type
   rules. Bad ones get the existing inline error row. Good ones go into a
   queue and are sent one after another: the next starts when the current
   upload's request finishes (`htmx:afterRequest` on the upload form),
   success or failure. A queued file shows a progress row reading "Waiting".
   Reuse `progressRow`, `uploadRow`, `sendUpload`; set the file on the form's
   input with `DataTransfer`.
4. **Picker too.** Add `multiple` to the file input so "File" in the Add sheet
   can pick several files; they use the same queue. Photo stays single (camera).
5. **Busy rule.** The existing "Wait for the file that is being added to
   finish" toast goes away: new files join the queue instead.
6. **Hint.** On desktop only (`(hover: hover) and (pointer: fine)`), a quiet
   line under the file count: "Drag files here to add them." Not shown on
   phones.
7. **Retry** keeps working for a single failed file.

## Not in this change

Moving files between folders by dragging; dropping onto a folder in the left
list; folder upload; phones (no file drag there).

## Checks

`tsc -p frontend/tsconfig.json`, `bin/build-assets`, `cargo fmt`, `clippy`,
`cargo test` (unchanged server; run to be safe). A Playwright check against a
local app: dispatch a synthetic drop of 3 files (one too big) on a folder page,
see one error row and two files uploaded in order, overlay shown and hidden.
Screenshots of the overlay at 1440px, light and dark.
