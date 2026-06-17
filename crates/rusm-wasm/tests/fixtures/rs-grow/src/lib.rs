//! Forces the wasm linear memory to grow well past its initial size (a 2 MiB heap
//! allocation it actually touches), then reports "grew" to the registered `collector`.
//! A successful report proves the host backed the grow; if a grow failed the instance
//! would trap before reporting. Lets a test count successful grows across many instances
//! on one engine without racing exit reasons.

#[rusm_rs::main]
fn main() {
    let mut buf = vec![0u8; 2 << 20]; // 2 MiB — forces linear-memory growth
    let last = buf.len() - 1;
    buf[last] = 1; // touch the far end so it's really committed
    std::hint::black_box(&buf);
    if let Some(collector) = rusm_rs::whereis("collector") {
        rusm_rs::send_bytes(collector, b"grew");
    }
}
