use fluent_templates::{ArcLoader, FluentLoader};
use loco_rs::controller::views::{
    engines::{self, TeraView},
    ViewRenderer,
};

/// Builds the view engine the same way `ViewEngineInitializer` does, including
/// the i18n `t()` and `asset_url()` functions the templates call.
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
    let read = engine
        .render(
            "chat/_receipt.html",
            serde_json::json!({
                "message": { "id": 42, "receipt": { "text": "Read by 2", "readers": ["bob", "carol"], "all": false } },
                "oob": true,
            }),
        )
        .expect("receipt should render");
    assert!(
        read.contains(r#"id="receipt-42" hx-swap-oob="true""#),
        "{read}"
    );
    assert!(read.contains(r#"title="Read by bob, carol""#), "{read}");
    assert!(
        read.contains(r##"data-toggle="#receipt-names-42""##),
        "{read}"
    );

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
}
