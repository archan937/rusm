//! Exercises `#[rusm_rs::service]`: the `calc` module's functions become a
//! `serve()` dispatch loop **and** a typed `Client`. To test both ends in one
//! component, `run` plays two roles by its first message: `"serve"` → run the
//! service; otherwise (a collector pid) → spawn a sibling `calc` and call it
//! through the generated client, forwarding the result.

#[rusm_rs::service]
pub mod calc {
    pub fn add(a: i64, b: i64) -> i64 {
        a + b
    }
    pub fn greet(name: String) -> String {
        format!("hi {name}")
    }
    // A streaming handler: its chunks ride a stream to the caller, who iterates.
    pub fn count_to(n: i64) -> impl Iterator<Item = i64> {
        1..=n
    }
    // A callback handler: `progress` stays in the caller; our calls travel back.
    pub fn work(progress: rusm_rs::Callback<i64>) -> String {
        for pct in [25, 50, 100] {
            progress.call(pct);
        }
        "done".to_string()
    }
}

wit_bindgen::generate!({
    world: "process",
    path: "wit",
    with: { "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor, "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv },
});

struct Component;

impl Guest for Component {
    fn run() {
        let first = rusm_rs::receive_bytes();
        if first == b"serve" {
            calc::serve(); // never returns
        }
        // Commander role: `first` is the collector pid (decimal).
        let collector = rusm_rs::Pid(String::from_utf8(first).unwrap().parse().unwrap());
        let client = calc::Client::spawn("calc").unwrap();
        rusm_rs::send_bytes(client.pid, b"serve"); // put the sibling into serve mode
        let sum = client.add(2, 3).unwrap();
        let hi = client.greet("RUSM".to_string()).unwrap();
        let nums: Vec<String> = client.count_to(3).map(|n| n.to_string()).collect();
        // A callback: the closure stays here, filling `seen` as the service reports.
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<i64>::new()));
        let sink = seen.clone();
        let status = client.work(move |pct| sink.borrow_mut().push(pct)).unwrap();
        let progress: Vec<String> = seen.borrow().iter().map(|n| n.to_string()).collect();
        rusm_rs::send_bytes(
            collector,
            format!(
                "sum={sum} {hi} count={} work={status} after {}",
                nums.join(","),
                progress.join("/"),
            )
            .as_bytes(),
        );
    }
}

export!(Component);
