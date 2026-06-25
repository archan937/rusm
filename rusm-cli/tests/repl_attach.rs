//! End-to-end: `rusm attach`'s JavaScript REPL over a real WebSocket. Boots a node
//! with the `WasmReplHost` wired in (as `rusm node start` does), attaches over
//! loopback, and drives stateful evaluation across the wire — the full path
//! protocol → node routing → REPL host → live js-runner → reply.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rusm_cli::WasmReplHost;
use rusm_node::{serve_on, ClientCommand, Node, ServerMessage};
use rusm_otp::Runtime;
use rusm_wasm::{CapabilityProfile, WasmRuntime};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Generous: the first eval lazily spawns the js-runner (a one-time compile).
const BUDGET: Duration = Duration::from_secs(30);

async fn send_eval(ws: &mut Ws, code: &str) {
    let cmd = ClientCommand::Eval { code: code.into() };
    ws.send(Message::Text(cmd.to_json().into())).await.unwrap();
}

/// Read frames until the next eval result, skipping the hello + telemetry snapshots.
async fn next_eval_result(ws: &mut Ws) -> (String, Vec<String>, Option<String>) {
    tokio::time::timeout(BUDGET, async {
        loop {
            let Some(Ok(Message::Text(text))) = ws.next().await else {
                panic!("connection closed before an eval result");
            };
            if let ServerMessage::EvalResult {
                value,
                output,
                error,
            } = ServerMessage::from_json(&text).unwrap()
            {
                return (value, output, error);
            }
        }
    })
    .await
    .expect("an eval result within the budget")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_evaluates_javascript_statefully_over_loopback() {
    let rt = Runtime::new();
    let wasm = Arc::new(WasmRuntime::new(rt.clone()).unwrap());
    let repl = Arc::new(WasmReplHost::new(Arc::clone(&wasm), rt.clone()));
    let node = Node::with_repl(rt.clone(), "test", 50, repl);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_on(listener, node));

    let (mut ws, _) = connect_async(format!("ws://{addr}")).await.unwrap();

    // A declaration binds but yields no value.
    send_eval(&mut ws, "const p = 41").await;
    let (value, _output, error) = next_eval_result(&mut ws).await;
    assert_eq!(error, None);
    assert_eq!(value, "");

    // The binding persists to the next line — the whole point of a stateful session.
    send_eval(&mut ws, "p + 1").await;
    let (value, _output, error) = next_eval_result(&mut ws).await;
    assert_eq!(error, None);
    assert_eq!(value, "42");

    // Console output is captured and returned alongside the value.
    send_eval(&mut ws, "console.log('hello'); p").await;
    let (value, output, error) = next_eval_result(&mut ws).await;
    assert_eq!(error, None);
    assert_eq!(output, ["hello"]);
    assert_eq!(value, "41");

    // A throw is reported as an error, and the session stays usable afterwards.
    send_eval(&mut ws, "throw new Error('boom')").await;
    let (_value, _output, error) = next_eval_result(&mut ws).await;
    assert_eq!(error.as_deref(), Some("Error: boom"));

    send_eval(&mut ws, "p * 2").await;
    let (value, _output, error) = next_eval_result(&mut ws).await;
    assert_eq!(error, None);
    assert_eq!(value, "82");

    // Keep the runtime alive until the end of the test.
    drop(wasm);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_can_call_a_resident_service_with_the_typed_client() {
    let rt = Runtime::new();
    let wasm = Arc::new(WasmRuntime::new(rt.clone()).unwrap());

    // A minimal resident "calc" service (CJS, as `rusm build` would emit): one
    // exported function the runner dispatches as an RPC handler.
    let caps = CapabilityProfile::Trusted.capabilities();
    wasm.register_js_component_with("calc", b"exports.add = (a, b) => a + b;".to_vec(), caps);
    let calc = wasm.spawn_registered("calc").expect("calc is registered");
    rt.register("calc", calc.pid());

    let repl = Arc::new(WasmReplHost::new(Arc::clone(&wasm), rt.clone()));
    let node = Node::with_repl(rt.clone(), "test", 50, repl);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_on(listener, node));

    let (mut ws, _) = connect_async(format!("ws://{addr}")).await.unwrap();

    // The killer demo: reach the running service by name and call it — a real
    // request/reply over the actor wire, driven from the shell.
    send_eval(&mut ws, "await connect('calc').add(2, 3)").await;
    let (value, _output, error) = next_eval_result(&mut ws).await;
    assert_eq!(error, None);
    assert_eq!(value, "5");

    drop(wasm);
}
