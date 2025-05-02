mod bindings {
    wit_bindgen::generate!({ generate_all });
}
use bindings::wex_examples::hello::handler::hello;

use std::io::Write as _;

use wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

wasi::http::proxy::export!(Handler);

struct Handler;

impl wasi::exports::http::incoming_handler::Guest for Handler {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let resp = OutgoingResponse::new(Fields::new());
        let body = resp.body().expect("failed to get outgoing body");

        let greeting = hello();

        ResponseOutparam::set(response_out, Ok(resp));

        let mut out = body.write().unwrap();
        out.write_all(greeting.as_bytes()).unwrap();
        out.flush().unwrap();
        drop(out);

        OutgoingBody::finish(body, None).unwrap();
    }
}
