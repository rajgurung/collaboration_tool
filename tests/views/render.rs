use fluent_templates::{ArcLoader, FluentLoader};
use loco_rs::controller::views::{
    engines::{self, TeraView},
    ViewRenderer,
};

/// Builds the view engine the same way `ViewEngineInitializer` does, including
/// the i18n `t()` function the templates call.
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
            "dashboard/index.html",
            serde_json::json!({
                "active": "tasks",
                "org": { "name": "Himalayan Ritual" },
                "me": { "username": "raj", "color": "#ffb454" },
            }),
        )
        .expect("app layout should render");

    assert!(rendered.contains("Himalayan Ritual"));
    assert!(rendered.contains("progress at a glance"));
    assert!(rendered
        .contains(r#"href="/tasks" class="tab-link flex items-center gap-2" aria-current="page""#));
    assert!(rendered.contains("<svg"), "icons should render");
}
