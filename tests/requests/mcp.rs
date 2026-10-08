use collab::app::App;
use loco_rs::testing::prelude::*;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn spike_tools_call_sees_parts() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request
            .post("/mcp")
            .add_header("accept", "application/json, text/event-stream")
            .add_header("host", "localhost")
            .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"whoami","arguments":{}}}))
            .await;
        println!("{} {}", res.status_code(), res.text());
        let res = request
            .post("/mcp")
            .add_header("accept", "application/json, text/event-stream")
            .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}))
            .await;
        println!("{} {}", res.status_code(), res.text());
        let res = request.get("/mcp").await;
        println!("GET {}", res.status_code());
    })
    .await;
}
