mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
       generate_all,
    });
    export!(Handler);
}

use bindings::wasi::messaging::request_reply::reply;
use bindings::wasi::messaging::types::{Error, Message};
use serde_json::json;

struct Handler;

impl bindings::exports::wasi::messaging::incoming_handler::Guest for Handler {
    fn handle(message: Message) -> Result<(), Error> {
        let params = message.data();
        let buf = serde_json::to_vec(&json!({
            "status_code": 200,
            "headers": [],
            "body": params,
        }))
        .map_err(|err| Error::Other(err.to_string()))?;
        reply(&message, Message::new(&buf))
    }
}
