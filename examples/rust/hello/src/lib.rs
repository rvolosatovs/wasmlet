mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
        with: {
            "wasmlet-examples:hello/handler": generate,
        },
    });
    export!(Handler);
}

use bindings::exports::wasmlet_examples::hello::handler::Guest;

struct Handler;

impl Guest for Handler {
    fn hello() -> String {
        "hello from Rust bindgen export".into()
    }
}
