mod imports {
    wasmtime::component::bindgen!({
        async: {
            only_imports: [
                "wasi:io/poll@0.2.5#[method]pollable.block",
                "wasi:io/poll@0.2.5#poll",
                "wasi:io/streams@0.2.5#[method]input-stream.blocking-read",
                "wasi:io/streams@0.2.5#[method]output-stream.blocking-flush",
                "wasi:io/streams@0.2.5#[method]output-stream.blocking-write-and-flush",
            ],
        },
        tracing: true,
        trappable_imports: true,
        require_store_data_send: true,
        with: {
            "wasi:cli/terminal-input/terminal-input": crate::engine::wasi::cli::TerminalInput,
            "wasi:cli/terminal-output/terminal-output": crate::engine::wasi::cli::TerminalOutput,
            "wasi:filesystem/types/descriptor": crate::engine::wasi::filesystem::Descriptor,
            "wasi:filesystem/types/directory-entry-stream": crate::engine::wasi::filesystem::DirectoryEntryStream,
            "wasi:http/types/fields": crate::engine::bindings::with::Fields,
            "wasi:http/types/future-incoming-response": crate::engine::wasi::http::FutureIncomingResponse,
            "wasi:http/types/future-trailers": crate::engine::wasi::http::FutureTrailers,
            "wasi:http/types/incoming-body": crate::engine::wasi::http::IncomingBody,
            "wasi:http/types/incoming-request": crate::engine::wasi::http::IncomingRequest,
            "wasi:http/types/incoming-response": crate::engine::wasi::http::IncomingResponse,
            "wasi:http/types/response-outparam": crate::engine::wasi::http::ResponseOutparam,
            "wasi:http/types/outgoing-body": crate::engine::bindings::with::OutgoingBody,
            "wasi:http/types/outgoing-request": crate::engine::wasi::http::OutgoingRequest,
            "wasi:http/types/outgoing-response": crate::engine::wasi::http::OutgoingResponse,
            "wasi:http/types/request-options": crate::engine::bindings::with::RequestOptions,
            "wasi:io/error/error": crate::engine::wasi::io::Error,
            "wasi:io/poll/pollable": crate::engine::wasi::io::Pollable,
            "wasi:io/streams/input-stream": crate::engine::wasi::io::InputStream,
            "wasi:io/streams/output-stream": crate::engine::wasi::io::OutputStream,
            "wasi:sockets/ip-name-lookup/resolve-address-stream": crate::engine::wasi::sockets::ResolveAddressStream,
            "wasi:sockets/network/network": crate::engine::wasi::sockets::Network,
            "wasi:sockets/tcp/tcp-socket": crate::engine::wasi::sockets::tcp::TcpSocket,
            "wasi:sockets/udp/incoming-datagram-stream": crate::engine::wasi::sockets::udp::IncomingDatagramStream,
            "wasi:sockets/udp/outgoing-datagram-stream": crate::engine::wasi::sockets::udp::OutgoingDatagramStream,
            "wasi:sockets/udp/udp-socket": crate::engine::wasi::sockets::udp::UdpSocket,
        }
    });
}

pub use imports::*;
pub mod exports {
    pub mod wasi {
        pub mod cli {
            mod bindings {
                wasmtime::component::bindgen!({
                    async: true,
                    world: "wasi:cli/command",
                    tracing: true,
                    trappable_imports: true,
                    with: {
                        "wasi": crate::engine::bindings::wasi,
                    }
                });
            }
            pub use bindings::{Command, CommandIndices, CommandPre};
        }

        pub mod http {
            mod bindings {
                wasmtime::component::bindgen!({
                    async: true,
                    world: "wasi:http/proxy",
                    tracing: true,
                    trappable_imports: true,
                    with: {
                        "wasi": crate::engine::bindings::wasi,
                    }
                });
            }
            pub use bindings::{Proxy, ProxyIndices, ProxyPre};
        }
    }
}

mod with {
    use std::sync::Arc;

    use crate::engine::{wasi, WithChildren};

    /// The concrete type behind a `wasi:http/types/outgoing-body` resource.
    pub type OutgoingBody = Arc<std::sync::Mutex<wasi::http::OutgoingBody>>;

    /// The concrete type behind a `wasi:http/types/fields` resource.
    pub type Fields = WithChildren<http::HeaderMap>;

    /// The concrete type behind a `wasi:http/types/request-options` resource.
    pub type RequestOptions = WithChildren<wasi::http::RequestOptions>;
}
