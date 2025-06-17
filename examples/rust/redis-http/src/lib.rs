mod bindings {
    wit_bindgen::generate!({
        ownership: Borrowing { duplicate_if_necessary: true },
        with: {
            "wasi:clocks/monotonic-clock@0.2.5": wasi::clocks::monotonic_clock,
            "wasi:io/error@0.2.5": wasi::io::error,
            "wasi:io/poll@0.2.5": wasi::io::poll,
            "wasi:io/streams@0.2.5": wasi::io::streams,
            "wasi:sockets/network@0.2.5": wasi::sockets::network,
            "wasi:sockets/tcp@0.2.5": wasi::sockets::tcp,
            "wasmlet-examples:redis/commands": generate,
            "wasmlet-examples:redis/database": generate,
            "wasmlet-examples:redis/resp3": generate,
            "wasmlet-examples:redis/pool": generate,
        },
    });
}
use std::collections::HashMap;

use anyhow::{anyhow, bail, Context as _};
use bindings::wasmlet_examples::redis::commands::{GetParam, SetOptions, SetParam};
use wasi::http::types::{
    ErrorCode, Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};
use wasi::sockets::network;

use crate::bindings::wasmlet_examples::redis::commands::{CommandParam, IncrParam};
use crate::bindings::wasmlet_examples::redis::database::{ActiveConnection, PendingConnection};
use crate::bindings::wasmlet_examples::redis::pool::{self, Connection};
use crate::bindings::wasmlet_examples::redis::resp3::{self, FrameValue, Value};

wasi::http::proxy::export!(Handler);

struct Handler;

fn set_err(out: ResponseOutparam, err: impl Into<String>) {
    ResponseOutparam::set(out, Err(ErrorCode::InternalError(Some(err.into()))));
}

fn connect() -> Result<(ActiveConnection, bool), network::ErrorCode> {
    match pool::get()? {
        Connection::Active(conn) => Ok((conn, true)),
        Connection::Pending(conn) => {
            conn.subscribe().block();
            let conn = PendingConnection::finish(conn)?;
            Ok((conn, false))
        }
    }
}

fn new_response(code: u16) -> anyhow::Result<OutgoingResponse> {
    let res = OutgoingResponse::new(Fields::new());
    if let Err(()) = res.set_status_code(code) {
        bail!("failed to set status code {code}");
    }
    Ok(res)
}

fn execute(conn: &ActiveConnection, cmd: CommandParam<'_>) -> anyhow::Result<Value> {
    if !conn
        .commands
        .feed(cmd)
        .map_err(|err| anyhow!(err).context("failed to feed command"))?
    {
        bail!("command buffer full")
    }
    let tx_poll = conn.commands.subscribe();
    conn.commands.write().context("failed to write")?;
    while conn.commands.buffered_bytes() > 0 {
        tx_poll.block();
        conn.commands.write().context("failed to write")?;
    }

    let rx_poll = conn.frames.subscribe();
    let reply = loop {
        conn.frames.read().context("failed to read")?;
        if let Some(frame) = conn.frames.next() {
            break frame;
        }
        rx_poll.block();
    };

    let reply =
        resp3::into_value(reply).map_err(|err| anyhow!(err).context("failed to parse value"))?;
    match reply {
        FrameValue::Value(value) => Ok(value),
        FrameValue::Push(..) => bail!("unexpected push received"),
    }
}

fn execute_get(conn: &ActiveConnection, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
    match execute(conn, CommandParam::Get(GetParam { key }))? {
        Value::BlobString(v) => Ok(Some(v)),
        Value::Null => Ok(None),
        v => bail!("unexpected GET response: {v:?}"),
    }
}

fn execute_incr(conn: &ActiveConnection, key: &str) -> anyhow::Result<i64> {
    match execute(conn, CommandParam::Incr(IncrParam { key }))? {
        Value::Number(v) => Ok(v),
        v => bail!("unexpected INCR response: {v:?}"),
    }
}

fn execute_set(conn: &ActiveConnection, key: &str, value: &str) -> anyhow::Result<()> {
    match execute(
        conn,
        CommandParam::Set(SetParam {
            key,
            value,
            ex: None,
            px: None,
            exat: None,
            pxat: None,
            options: SetOptions::empty(),
        }),
    )? {
        Value::SimpleString(buf) if buf == b"OK" => Ok(()),
        v => bail!("unexpected SET response: {v:?}"),
    }
}

fn handle(request: IncomingRequest) -> anyhow::Result<(OutgoingResponse, String)> {
    let (conn, pooled) = connect().context("failed to connect")?;
    let pq = request.path_with_query();
    let pq = pq.as_deref().unwrap_or("/");
    let (path, params) = pq.split_once('?').unwrap_or((pq, ""));
    let params: HashMap<&str, &str> = params
        .split("&")
        .filter_map(|p| p.split_once("="))
        .collect();
    match path {
        "/test" => {
            let key = if pooled { "pooled" } else { "unpooled" };
            match execute_incr(&conn, key) {
                Ok(n) => {
                    pool::put(conn);
                    let res = new_response(200)?;
                    Ok((res, format!("{key}: {n}")))
                }
                Err(err) => {
                    let res = new_response(500)?;
                    Ok((res, format!("{err:#}")))
                }
            }
        }
        "/incr" => {
            let Some(key) = params.get("key") else {
                let res = new_response(400)?;
                return Ok((res, "`key` parameter missing".into()));
            };
            match execute_incr(&conn, key) {
                Ok(n) => {
                    pool::put(conn);
                    let res = new_response(200)?;
                    Ok((res, n.to_string()))
                }
                Err(err) => {
                    let res = new_response(500)?;
                    Ok((res, format!("{err:#}")))
                }
            }
        }
        "/get" => {
            let Some(key) = params.get("key") else {
                let res = new_response(400)?;
                return Ok((res, "`key` parameter missing".into()));
            };
            match execute_get(&conn, key) {
                Ok(Some(value)) => {
                    pool::put(conn);
                    let res = new_response(200)?;
                    Ok((res, String::from_utf8_lossy(&value).into()))
                }
                Ok(None) => {
                    pool::put(conn);
                    let res = new_response(404)?;
                    Ok((res, format!("`{key}` not found")))
                }
                Err(err) => {
                    let res = new_response(500)?;
                    Ok((res, format!("{err:#}")))
                }
            }
        }
        "/set" => {
            let Some(key) = params.get("key") else {
                let res = new_response(400)?;
                return Ok((res, "`key` parameter missing".into()));
            };
            let Some(value) = params.get("value") else {
                let res = new_response(400)?;
                return Ok((res, "`value` parameter missing".into()));
            };
            match execute_set(&conn, key, value) {
                Ok(()) => {
                    pool::put(conn);
                    let res = new_response(200)?;
                    Ok((res, String::default()))
                }
                Err(err) => {
                    let res = new_response(500)?;
                    Ok((res, format!("{err:#}")))
                }
            }
        }
        _ => {
            let res = new_response(404)?;
            Ok((res, format!("endpoint `{path}` unknown")))
        }
    }
}

impl wasi::exports::http::incoming_handler::Guest for Handler {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let (res, buf) = match handle(request) {
            Ok((res, buf)) => (res, buf),
            Err(err) => {
                set_err(response_out, format!("{err:#}"));
                return;
            }
        };
        let body = match res.body() {
            Ok(body) => body,
            Err(()) => {
                set_err(response_out, "failed to get response body");
                return;
            }
        };

        ResponseOutparam::set(response_out, Ok(res));
        {
            let stream = match body.write() {
                Ok(stream) => stream,
                Err(()) => {
                    eprintln!("failed to get response body stream handle");
                    return;
                }
            };
            if let Err(err) = stream.blocking_write_and_flush(buf.as_bytes()) {
                eprintln!("failed to write response body: {err}");
                return;
            }
        }
        if let Err(err) = OutgoingBody::finish(body, None) {
            eprintln!("failed to finish response body: {err}");
        }
    }
}
