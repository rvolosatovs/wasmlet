mod bindings {
    use crate::Handler;

    wit_bindgen::generate!({
        with: {
            "wasi:io/poll@0.2.5": wasi::io::poll,
            "wex-examples:db/handler": generate,
        },
    });
    export!(Handler);
}

use bindings::exports::wex_examples::db::handler;


use std::sync::OnceLock;

use wasi::io::poll::Pollable;
use wasi::io::streams::{InputStream, OutputStream};
use wasi::sockets::network::{self, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress};
use wasi::sockets::tcp_create_socket::create_tcp_socket;
use wasi::sockets::{instance_network::instance_network, tcp::TcpSocket};

struct Handler;

enum Socket {
    Connecting(TcpSocket),
    Connected(TcpSocket, InputStream, OutputStream),
    Error(network::ErrorCode),
}

static SOCKET: OnceLock<Socket> = OnceLock::new();

struct Database;
struct GetFuture;
struct SetFuture;

impl handler::GuestSetFuture for SetFuture {
    fn subscribe(&self) -> Pollable {
        todo!()
    }

    fn await_(&self) -> Result<Option<String>, ()> {
        todo!()
    }
}

impl handler::GuestGetFuture for GetFuture {
    fn subscribe(&self) -> Pollable {
        todo!()
    }

    fn await_(&self) -> Result<String, ()> {
        todo!()
    }
}

impl handler::GuestDatabase for Database {
    fn subscribe(&self) -> Pollable {
        let sock = SOCKET.get().expect("invalid socket state");
        match sock {
            Socket::Connecting(sock) => sock.subscribe(),
            Socket::Connected(sock, ..) => sock.subscribe(),
            Socket::Error(error_code) => todo!(),
        }
    }

    fn get(&self, k: String) -> handler::GetFuture {
        todo!()
    }

    fn set(&self, k: String, v: String) -> handler::SetFuture {
        todo!()
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        eprintln!("DROP DB");
    }
}

impl handler::Guest for Handler {
    type Database = Database;
    type GetFuture = GetFuture;
    type SetFuture = SetFuture;

    fn connect() -> Result<handler::Database, ()> {
        if let Socket::Error(err) = SOCKET.get_or_init(|| {
            let net = instance_network();
            let sock = match create_tcp_socket(IpAddressFamily::Ipv4) {
                Ok(sock) => sock,
                Err(err) => return Socket::Error(err),
            };
            if let Err(err) = sock.start_connect(
                &net,
                IpSocketAddress::Ipv4(Ipv4SocketAddress {
                    address: (127, 0, 0, 1),
                    port: 9000,
                }),
            ) {
                return Socket::Error(err);
            };
            Socket::Connecting(sock)
        }) {
            eprintln!("connection failed: {err:?}");
            return Err(());
        };
        Ok(handler::Database::new(Database))
    }
}
