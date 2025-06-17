mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
        with: {
            "wasi:clocks/monotonic-clock@0.2.5": wasi::clocks::monotonic_clock,
            "wasi:io/error@0.2.5": wasi::io::error,
            "wasi:io/poll@0.2.5": wasi::io::poll,
            "wasi:io/streams@0.2.5": wasi::io::streams,
            "wasi:sockets/network@0.2.5": wasi::sockets::network,
            "wasi:sockets/tcp@0.2.5": wasi::sockets::tcp,
            "wasmlet-examples:hello/handler": generate,
            "wasmlet-examples:redis/commands": generate,
            "wasmlet-examples:redis/database": generate,
            "wasmlet-examples:redis/resp3": generate,
            "wasmlet-examples:redis/pool": generate,
        },
    });
    export!(Handler);
}

use bindings::wasmlet_examples::redis::commands::{Command, Get, Hello, Ping};
use bindings::wasmlet_examples::redis::database::PendingConnection;
use bindings::wasmlet_examples::redis::pool;
use bindings::wasmlet_examples::redis::pool::Connection;
use bindings::wasmlet_examples::redis::resp3::{self, FrameValue, IndirectValue, Value};

struct Handler;

impl bindings::exports::wasmlet_examples::hello::handler::Guest for Handler {
    fn hello() -> String {
        let conn = match pool::get() {
            // TODO: Incr
            Ok(Connection::Active(conn)) => conn,
            Ok(Connection::Pending(conn)) => {
                eprintln!("cache miss");
                conn.subscribe().block();
                PendingConnection::finish(conn).expect("failed to connect")
            }
            Err(err) => {
                return format!("err: {err:?}");
            }
        };
        let cmds = [
            Command::Hello(Hello {
                protover: None,
                auth: None,
                setname: None,
            }),
            Command::Ping(Ping { message: None }),
            Command::Ping(Ping {
                message: Some("hello".into()),
            }),
            Command::Get(Get { key: "f2".into() }),
        ];
        let n = conn.commands.feed_many(&cmds).expect("failed to feed cmds");
        assert_eq!(n, 4);

        let tx_poll = conn.commands.subscribe();
        while conn.commands.buffered_bytes() > 0 {
            tx_poll.block();
            conn.commands.write().expect("failed to write");
        }

        let rx_poll = conn.frames.subscribe();
        let mut replies = Vec::with_capacity(4);
        while replies.len() != 4 {
            rx_poll.block();
            for frame in conn.frames.next_many(4 - replies.len() as u32) {
                let frame = resp3::into_value(frame).expect("failed to parse value");
                replies.push(frame);
            }
            conn.frames.read().expect("failed to read");
        }
        pool::put(conn);

        match replies.pop().unwrap() {
            FrameValue::Value(Value::BlobString(v)) => return format!("SIMPLE STRING {v:?}"),
            FrameValue::Value(Value::Array(vs)) => {
                for v in vs {
                    eprintln!("GET : {:?}", IndirectValue::unwrap(v));
                }
            }
            FrameValue::Value(v) => return format!("Unexpected GET res {v:?}"),
            FrameValue::Push(..) => panic!("invalid GET response"),
        }
        format!("got replies: {replies:?}")
    }
}
