use fluent_templates::{ArcLoader, FluentLoader};
use loco_rs::controller::views::{
    engines::{self, TeraView},
    ViewRenderer,
};

/// Builds the view engine the same way `ViewEngineInitializer` does, including
/// the i18n `t()`, `asset_url()` and `org_url()` functions the templates call.
fn view_engine() -> TeraView {
    let loader = std::sync::Arc::new(
        ArcLoader::builder("assets/i18n", unic_langid::langid!("en-US"))
            .shared_resources(Some(&["assets/shared.ftl".into()]))
            .customize(|bundle| bundle.set_use_isolating(false))
            .build()
            .expect("locales should load"),
    );

    engines::TeraView::build_with_post_process(move |tera| {
        tera.register_function("t", FluentLoader::new(loader.clone()));
        tera.register_function("asset_url", collab::initializers::view_engine::asset_url);
        tera.register_function("org_url", collab::initializers::view_engine::org_url);
        Ok(())
    })
    .expect("view engine should build")
}

#[test]
fn renders_home_view_with_i18n() {
    let rendered = view_engine()
        .render("home/index.html", serde_json::json!({}))
        .expect("home view should render");

    assert!(
        rendered.contains("Collaboration Tool"),
        "expected the i18n key to resolve, got: {rendered}"
    );
    assert!(rendered.contains("/static/css/app.css"));
}

#[test]
fn renders_app_layout_with_nav() {
    let rendered = view_engine()
        .render(
            "members/index.html",
            serde_json::json!({
                "active": "tasks",
                "org": { "name": "Himalayan Ritual" },
                "me": { "username": "raj", "color": "#ffb454" },
                "active_members": [],
                "pending": [],
                "join_url": "http://localhost:5150/join/himalayan-ritual",
            }),
        )
        .expect("app layout should render");

    assert!(rendered.contains("Himalayan Ritual"));
    assert!(rendered.contains("<h1>Members</h1>"));
    // Desktop sidebar and mobile tab bar both mark Tasks as current.
    assert!(rendered.contains(r#"href="/tasks" class="side-link" aria-current="page""#));
    assert!(rendered.contains(r#"href="/tasks" class="tab" aria-current="page""#));
    assert!(rendered.contains("<svg"), "icons should render");
}

#[test]
fn renders_read_receipts_as_live_swaps() {
    let engine = view_engine();
    let person = |name: &str| serde_json::json!({ "id": 1, "username": name, "color": "#ffb454" });
    let readers: Vec<_> = ["bob", "carol", "dev", "eve", "fay"]
        .into_iter()
        .map(person)
        .collect();
    let read = engine
        .render(
            "chat/_receipt.html",
            serde_json::json!({
                "message": { "id": 42, "receipt": {
                    "dm": false,
                    "text": "Read by bob, carol, dev, eve, fay",
                    "readers": readers,
                    "shown": readers[..3],
                    "more": 2,
                    "all": true,
                }, "own": true },
                "oob": true,
            }),
        )
        .expect("receipt should render");
    assert!(
        read.contains(r#"id="receipt-42" hx-swap-oob="true""#),
        "{read}"
    );
    assert!(
        read.contains(r#"aria-label="Read by bob, carol, dev, eve, fay""#),
        "{read}"
    );
    assert_eq!(read.matches("avatar avatar-2xs").count(), 3, "{read}");
    assert!(
        read.contains(r#"<span class="seen-more">+2</span>"#),
        "{read}"
    );
    assert!(read.contains("Read by 5</span>"), "{read}");
    assert_eq!(read.matches("receipt-pop-row").count(), 5, "{read}");
    // The tick is swapped with the faces.
    assert!(
        read.contains(r#"<span class="tick tick-read" id="tick-42" hx-swap-oob="true">"#),
        "{read}"
    );
    assert!(read.contains("read by everyone"), "{read}");
    assert!(
        read.contains(r##"data-toggle="#receipt-names-42""##),
        "{read}"
    );
    assert!(!read.contains("title="), "the popover replaces tooltips");
    // 3 stacked avatars and 5 list rows, none read out again by screen readers.
    assert_eq!(
        read.matches(r#"aria-hidden="true" class="avatar"#).count(),
        8,
        "{read}"
    );
    assert!(read.contains(r#"aria-expanded="false""#), "{read}");

    // Someone who did not write it gets the same faces, and no tick: their
    // page has none to swap.
    let theirs = engine
        .render(
            "chat/_receipt.html",
            serde_json::json!({
                "message": { "id": 42, "receipt": {
                    "dm": false, "text": "Read by bob", "readers": [person("bob")],
                    "shown": [person("bob")], "more": 0, "all": false,
                }, "own": false },
                "oob": true,
            }),
        )
        .expect("receipt should render");
    assert!(theirs.contains(r#"aria-label="Read by bob""#), "{theirs}");
    assert!(!theirs.contains("tick"), "{theirs}");

    let dm = engine
        .render(
            "chat/_receipt.html",
            serde_json::json!({ "message": { "id": 43, "receipt": {
                "dm": true, "text": "Read", "readers": [person("bob")], "shown": [person("bob")], "more": 0, "all": true,
            }, "own": true }, "oob": true }),
        )
        .expect("a DM receipt should render");
    // DMs: no faces; "Read" under the message, and the tick.
    assert!(
        dm.contains(r#"<span class="msg-receipt" id="receipt-43" hx-swap-oob="true"><span class="msg-receipt-dm">"#),
        "{dm}"
    );
    assert!(dm.contains("Read</span></span>"), "{dm}");
    assert!(
        dm.contains(r#"class="tick tick-read" id="tick-43""#),
        "{dm}"
    );
    assert!(dm.contains(", read</span>"), "{dm}");
    assert!(!dm.contains("avatar"), "{dm}");

    let unread = engine
        .render(
            "chat/_receipt.html",
            serde_json::json!({ "message": { "id": 42, "receipt": null } }),
        )
        .expect("an empty slot should render");
    assert!(
        unread.contains(r#"<span class="msg-receipt" id="receipt-42"></span>"#),
        "{unread}"
    );
    assert!(
        !unread.contains("tick"),
        "the page renders the tick in the bubble"
    );

    let grey = engine
        .render(
            "chat/_tick.html",
            serde_json::json!({ "message": { "id": 42, "receipt": null } }),
        )
        .expect("an unread tick should render");
    assert!(
        grey.contains(r#"<span class="tick" id="tick-42">"#),
        "{grey}"
    );
    assert!(grey.contains("not read yet"), "{grey}");
}

#[test]
fn renders_reactions_as_live_swaps() {
    let engine = view_engine();
    let reactor = |name: &str, username: &str| serde_json::json!({ "name": name, "username": username, "color": "#ffb454" });
    let pick = |emoji: &str, mine: bool| serde_json::json!({ "emoji": emoji, "mine": mine });
    let html = engine
        .render(
            "chat/_reactions_update.html",
            serde_json::json!({
                "message": {
                    "id": 42,
                    "conversation_id": 7,
                    "reactions": [{
                        "emoji": "👍", "count": 2, "mine": true,
                        "people": [reactor("You", "alice"), reactor("bob", "bob")],
                        "names": "You, bob", "label": "👍 2, you reacted",
                    }],
                    "picks": [pick("👍", true), pick("❤️", false)],
                },
                "oob": true,
            }),
        )
        .expect("reactions should render");
    assert!(
        html.contains(r#"id="reacts-42" role="group" aria-label="Reactions" aria-live="off" hx-swap-oob="true""#),
        "{html}"
    );
    assert!(html.contains(r#"id="tools-42""#), "{html}");
    // Only the six reactions are swapped, so Reply in thread stays put.
    assert!(
        html.contains(
            r#"<span class="tool-picks" id="tools-42" aria-live="off" hx-swap-oob="true">"#
        ),
        "{html}"
    );
    assert!(!html.contains("Reply in thread"), "{html}");
    assert!(
        html.contains(r#"data-url="/chat/7/messages/42/reactions""#),
        "{html}"
    );
    assert!(html.contains(r#"class="react mine""#), "{html}");
    assert!(html.contains(r#"aria-label="👍 2, you reacted""#), "{html}");
    assert!(html.contains(r#"title="Remove your 👍""#), "{html}");
    assert!(html.contains(r#"title="React ❤️""#), "{html}");
    // The avatar is alice's; the row says "You".
    assert!(html.contains(r#"<span>You</span>"#), "{html}");

    let empty = engine
        .render(
            "chat/_reactions.html",
            serde_json::json!({ "message": { "id": 42, "conversation_id": 7, "reactions": [] } }),
        )
        .expect("an empty slot should render");
    // Nothing inside, not even a space, so `.reacts:empty` hides it.
    assert!(
        empty.contains(r#"<div class="reacts" id="reacts-42" role="group" aria-label="Reactions" aria-live="off"></div>"#),
        "{empty}"
    );
}

#[test]
fn the_action_sheet_escapes_the_message() {
    let pick = |emoji: &str| serde_json::json!({ "emoji": emoji, "mine": false });
    let html = view_engine()
        .render(
            "chat/_actions.html",
            serde_json::json!({
                "message": {
                    "id": 42, "conversation_id": 7, "author": "<i>bob</i>", "color": "#ffb454",
                    "at": "10:31", "own": false, "body": "<b>x</b> & <script>alert(1)</script>",
                    "reactions": [], "picks": [pick("👍")],
                },
            }),
        )
        .expect("the sheet should render");
    assert!(
        html.contains("&lt;b&gt;x&lt;/b&gt; &amp; &lt;script&gt;"),
        "{html}"
    );
    assert!(
        !html.contains("<b>x</b>") && !html.contains("<script>"),
        "{html}"
    );
    assert!(!html.contains("<i>bob</i>"), "{html}");
}

/// The live reply line swaps the parent's slot, so it is rendered with the
/// parent's id, never the reply's.
#[test]
fn renders_the_reply_line_into_the_parents_slot() {
    let engine = view_engine();
    let face = |name: &str| serde_json::json!({ "id": 1, "username": name, "color": "#ffb454" });
    let html = engine
        .render(
            "chat/_thread_link.html",
            serde_json::json!({
                "message": {
                    "id": 481,
                    "thread": {
                        "url": "/chat/12/threads/481", "count": 128, "label": "128 replies",
                        "last": "Yesterday",
                        "faces": [face("raj"), face("<b>s</b>"), face("samir")], "more": 1,
                    },
                },
                "oob": true,
            }),
        )
        .expect("the reply line should render");
    assert!(
        html.contains(r#"id="thread-481" aria-live="off" hx-swap-oob="true""#),
        "{html}"
    );
    assert!(html.contains(r#"href="/chat/12/threads/481""#), "{html}");
    assert!(html.contains("<b>128 replies</b>"), "{html}");
    assert!(html.contains("Yesterday"), "{html}");
    assert!(
        html.contains(r#"<span class="thread-more">+1</span>"#),
        "{html}"
    );
    assert!(!html.contains("<b>s</b>"), "names are escaped: {html}");

    // No replies: an empty slot, so `.thread-slot:empty` hides it and the
    // first reply still has a target.
    let empty = engine
        .render(
            "chat/_thread_link.html",
            serde_json::json!({ "message": { "id": 481, "thread": null } }),
        )
        .expect("an empty slot should render");
    assert!(
        empty.contains(r#"<span class="thread-slot" id="thread-481" aria-live="off"></span>"#),
        "{empty}"
    );
}

#[test]
fn renders_the_thread_count_as_a_live_swap() {
    let engine = view_engine();
    let html = engine
        .render(
            "chat/_thread_count.html",
            serde_json::json!({ "count": 2, "count_label": "2 replies", "oob": true }),
        )
        .expect("the count should render");
    assert!(
        html.contains(r#"id="thread-count" aria-live="off" hx-swap-oob="true">2 replies</div>"#),
        "{html}"
    );
    let none = engine
        .render(
            "chat/_thread_count.html",
            serde_json::json!({ "count": 0, "count_label": "0 replies" }),
        )
        .expect("an empty count should render");
    assert!(none.contains(r#"aria-live="off"></div>"#), "{none}");
}

/// Every live swap target keeps a home: the time, tick and thread footer sit
/// inside the bubble; chips and reader faces share a row after it; then the
/// toolbar.
#[test]
fn renders_a_message_as_one_block() {
    let engine = view_engine();
    let face = |name: &str| serde_json::json!({ "id": 1, "username": name, "color": "#ffb454" });
    let reactor =
        |name: &str| serde_json::json!({ "name": name, "username": name, "color": "#ffb454" });
    let chip = |emoji: &str| serde_json::json!({ "emoji": emoji, "count": 1, "mine": false, "people": [reactor("bob")], "names": "bob", "label": format!("{emoji} 1") });
    let pick = |emoji: &str| serde_json::json!({ "emoji": emoji, "mine": false });
    let message = |own: bool, read_receipts: bool| {
        serde_json::json!({
            "message": {
                "id": 42, "conversation_id": 7, "author": "alice", "color": "#ffb454",
                "parts": [{ "text": "Stock count Friday", "mention": false }],
                "at": "09:31", "own": own, "start": true, "show_time": true,
                "read_receipts": read_receipts, "receipt": null,
                "reactions": [chip("👍"), chip("✅")], "picks": [pick("👍")],
                "can_reply": true,
                "thread": {
                    "url": "/chat/7/threads/42", "count": 3, "label": "3 replies",
                    "last": "14:05", "faces": [face("bob")], "more": 0,
                },
            },
        })
    };
    let html = engine
        .render("chat/_message.html", message(true, true))
        .expect("a message should render");
    let at = |needle: &str| {
        html.find(needle)
            .unwrap_or_else(|| panic!("missing {needle}: {html}"))
    };
    let bubble = at(r#"class="bubble""#);
    let stamp = at(r#"<span class="stamp"><span class="sr-only">Sent </span>09:31"#);
    let tick = at(r#"<span class="tick" id="tick-42">"#);
    let thread = at(r#"id="thread-42""#);
    let bubble_end = thread + html[thread..].find("</div>").expect("the bubble closes");
    let reacts = at(r#"id="reacts-42""#);
    let receipt = at(r#"id="receipt-42""#);
    let tools = at(r#"id="tools-42""#);
    assert!(bubble < stamp && stamp < tick && tick < thread, "{html}");
    assert!(html.contains("stamp-pad stamp-pad-tick"), "{html}");
    let foot = at(r#"class="msg-foot""#);
    assert!(foot < reacts, "your chips and faces share one row: {html}");
    assert!(html[thread..bubble_end].contains("thread-foot"), "{html}");
    assert!(
        bubble_end < reacts && reacts < receipt && receipt < tools,
        "{html}"
    );
    assert!(!html.contains("msg-time"), "{html}");

    // Someone else's message in a group: the same row of chips and faces,
    // but no tick.
    let others = engine
        .render("chat/_message.html", message(false, true))
        .expect("someone else's message should render");
    assert!(
        others.contains(r#"<span class="stamp">09:31</span>"#),
        "{others}"
    );
    assert!(!others.contains("stamp-pad-tick"), "{others}");
    assert!(!others.contains(r#"id="tick-"#), "{others}");
    let at = |needle: &str| {
        others
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle}: {others}"))
    };
    let (foot, reacts, receipt) = (
        at(r#"class="msg-foot""#),
        at(r#"id="reacts-42""#),
        at(r#"id="receipt-42""#),
    );
    assert!(foot < reacts && reacts < receipt, "{others}");

    // Someone else's message in a DM: chips only.
    let dm = engine
        .render("chat/_message.html", message(false, false))
        .expect("someone else's DM should render");
    assert!(!dm.contains(r#"id="receipt-"#), "{dm}");
    assert!(!dm.contains(r#"id="tick-"#), "{dm}");
}

/// One task note: the quick bar and the chips post to the note's own route,
/// every button is a plain button, and the text is escaped.
#[test]
fn renders_a_note_with_reactions() {
    let reactor =
        |name: &str| serde_json::json!({ "name": name, "username": name, "color": "#ffb454" });
    let pick = |emoji: &str, mine: bool| serde_json::json!({ "emoji": emoji, "mine": mine });
    let html = view_engine()
        .render(
            "tasks/_note.html",
            serde_json::json!({
                "note": {
                    "id": 5, "task_id": 3, "author": "bob", "color": "#ffb454",
                    "parts": [{ "text": "<b>Shelves</b> done", "mention": false }],
                    "at": "Thu 15 Oct, 09:31",
                    "reactions": [{
                        "emoji": "👍", "count": 1, "mine": true, "people": [reactor("You")],
                        "names": "You", "label": "👍 1, you reacted",
                    }],
                    "picks": [pick("👍", true), pick("❤️", false), pick("😂", false), pick("🎉", false), pick("👀", false), pick("✅", false)],
                },
            }),
        )
        .expect("a note should render");
    assert!(
        html.contains(r#"<article class="note" id="note-5">"#),
        "{html}"
    );
    assert_eq!(
        html.matches(r#"hx-post="/tasks/3/notes/5/reactions""#)
            .count(),
        7,
        "six picks and one chip: {html}"
    );
    assert_eq!(
        html.matches(r#"hx-sync="closest .note:replace""#).count(),
        7,
        "{html}"
    );
    assert!(
        html.contains(r#"<div class="quick-react hidden" id="note-picks-5""#),
        "{html}"
    );
    assert!(
        html.contains(r#"<div class="note-who list hidden" id="note-who-5">"#),
        "{html}"
    );
    assert_eq!(
        html.matches("<button").count(),
        html.matches(r#"<button type="button""#).count(),
        "every button is type=button: {html}"
    );
    assert!(
        html.contains("&lt;b&gt;Shelves&lt;&#x2F;b&gt; done")
            || html.contains("&lt;b&gt;Shelves&lt;/b&gt; done"),
        "{html}"
    );
    assert!(!html.contains("<b>Shelves</b>"), "{html}");
}
