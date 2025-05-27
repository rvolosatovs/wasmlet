use wasi::http::types::{IncomingRequest, ResponseOutparam};

wasi::http::proxy::export!(Handler);

struct Handler;

impl wasi::exports::http::incoming_handler::Guest for Handler {
    fn handle(_request: IncomingRequest, _response_out: ResponseOutparam) {
        loop {}
    }
}
