# WebSocket spike (Task 4a)

Date: 2026-10-07. Loco 1.2.0, axum 0.8 (`ws` feature), HTMX 2.0.11, htmx-ext-ws 2.0.4.

## Result

Works end to end.

- A Loco route can take axum's `WebSocketUpgrade` extractor next to `State<AppContext>`
  and return `ws.on_upgrade(...)` as the `Response`. Loco 1.2 has no WebSocket layer of
  its own, so we use axum directly.
- One `tokio::sync::broadcast::Sender<String>` stored in `ctx.shared_store` (inserted in
  `Hooks::after_context`) fans messages out to every connection.
- Each connection runs one `tokio::select!` loop over `socket.recv()` and `rx.recv()`.
  No `tokio::spawn` is needed.
- Two Node clients: a message from A reached A and B in 7 ms.
- Headless Chrome with `hx-ext="ws" ws-connect="..."` and a `<form ws-send>`: the form
  was sent, a separate Node client received the broadcast, and the browser swapped the
  HTML into `#messages`.

## How the HTMX ws extension talks

- Browser to server: `ws-send` on a form sends one JSON text frame with the form fields
  plus a `HEADERS` object (`HX-Trigger`, `HX-Current-URL`, ...).
  Example: `{"body": "hi", "HEADERS": {...}}`.
- Server to browser: send an HTML fragment. Elements with `hx-swap-oob` are swapped by
  `id`, e.g. `<div id="messages" hx-swap-oob="beforeend">...</div>` appends.

## Decisions for Task 13 (live chat)

- Hub: `broadcast` channel per organisation (or one channel carrying
  `(conversation_id, html)` and each socket filters to conversations it belongs to).
  Membership must be checked when the socket opens and on every send.
- Auth: the browser sends same-origin cookies on the WebSocket handshake, so the JWT
  cookie extractor can run on the upgrade request. Verify in Task 13.
- Security: WebSockets are not covered by CORS or SameSite in the same way as forms.
  Check the `Origin` header against the configured host on upgrade to block
  cross-site WebSocket hijacking.
- Escape all user text before it goes into HTML (the spike used a manual escape; the
  real code should render through Tera so escaping is automatic).
- Lagged receivers (`RecvError::Lagged`) skip messages. Clients should reload the
  message list on reconnect.
- Routes are exact: `/spike` matched, `/spike/` returned 404.
