use std::collections::VecDeque;
use std::sync::Mutex;

use wasi::sockets::network;

use crate::bindings::exports::wasmlet_examples::redis::database::{ActiveConnection, Guest as _};
use crate::bindings::exports::wasmlet_examples::redis::pool::{Connection, Guest};
use crate::Handler;

static POOL: Mutex<VecDeque<ActiveConnection>> = Mutex::new(VecDeque::new());

impl Guest for Handler {
    fn get() -> Result<Connection, network::ErrorCode> {
        if let Ok(Some(conn)) = POOL.try_lock().as_deref_mut().map(VecDeque::pop_front) {
            Ok(Connection::Active(conn))
        } else {
            let conn = Handler::connect()?;
            Ok(Connection::Pending(conn))
        }
    }

    fn put(connection: ActiveConnection) {
        if let Ok(pool) = POOL.try_lock().as_deref_mut() {
            pool.push_back(connection)
        }
    }
}
