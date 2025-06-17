use crate::Handler;

wit_bindgen::generate!({
    with: {
        "wasi:clocks/monotonic-clock@0.2.5": wasi::clocks::monotonic_clock,
        "wasi:io/error@0.2.5": wasi::io::error,
        "wasi:io/poll@0.2.5": wasi::io::poll,
        "wasi:io/streams@0.2.5": wasi::io::streams,
        "wasi:sockets/network@0.2.5": wasi::sockets::network,
        "wasi:sockets/tcp@0.2.5": wasi::sockets::tcp,
        "wasmlet-examples:redis/commands": generate,
        "wasmlet-examples:redis/database": generate,
        "wasmlet-examples:redis/pool": generate,
        "wasmlet-examples:redis/resp3": generate,
    },
});
export!(Handler);
