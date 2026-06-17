//! A Rust guest that loads JS at runtime: it spawns a dynamic instance of the "runner"
//! template with an **inline** JS bundle (the plain-string source). The loaded JS runs on
//! the js-runner under the template's declared profile and messages the collector —
//! proving a Rust guest drives spawn-from. `#[rusm_rs::main]` hides the component shell.

#[rusm_rs::main]
fn main() {
    let inner = r#"module.exports.default = async function () {
        Process.send(Process.whereis("collector"), "ran from rs");
    };"#;
    if let Err(e) = rusm_rs::spawn_from("runner", &format!("inline:{inner}")) {
        if let Some(collector) = rusm_rs::whereis("collector") {
            rusm_rs::send_bytes(collector, format!("err: {e}").as_bytes());
        }
    }
}
