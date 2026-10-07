use collab::{
    app::App,
    workers::resend_email::{Worker, WorkerArgs},
};
use loco_rs::{bgworker::BackgroundWorker, testing::prelude::*};
use serial_test::serial;

/// Without a key the worker refuses instead of calling Resend.
#[tokio::test]
#[serial]
async fn refuses_without_an_api_key() {
    let boot = boot_test::<App>().await.unwrap();
    let result = Worker::build(&boot.app_context)
        .perform(WorkerArgs {
            from: "a@example.com".to_string(),
            to: "b@example.com".to_string(),
            subject: "s".to_string(),
            text: "t".to_string(),
            html: "h".to_string(),
        })
        .await;
    assert!(result.unwrap_err().to_string().contains("RESEND_API_KEY"));
}
