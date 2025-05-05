mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
        with: {
            "wex-examples:hello/handler": generate,
        },
    });
    export!(Handler);
}

use bindings::exports::wex_examples::hello::handler::Guest;

struct Handler;

impl Guest for Handler {
    fn hello() -> String {
        "hello from Rust".into()
    }
}
