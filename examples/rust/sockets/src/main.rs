use std::io::Write as _;
use std::net::TcpListener;

use anyhow::Context as _;

fn main() -> anyhow::Result<()> {
    eprintln!("binding on 127.0.0.1:8080");
    let sock = TcpListener::bind("127.0.0.1:8080")?;
    let addr = sock.local_addr().context("failed to get socket address")?;
    eprintln!("sucessfully bound socket on addr `{addr}`");
    loop {
        eprintln!("accepting connection...");
        match sock.accept() {
            Ok((mut conn, addr)) => {
                eprintln!("accepted conn from {addr}");
                if let Err(err) = conn.write_all(b"hello") {
                    eprintln!("failed to write `hello`: {err}");
                }
            }
            Err(err) => {
                eprintln!("failed to accept conn: {err}");
            }
        }
    }
}
