use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use wasmtime::component::Resource;

use crate::engine::bindings::wasi::cli::{
    environment, exit, stderr, stdin, stdout, terminal_input, terminal_output, terminal_stderr,
    terminal_stdin, terminal_stdout,
};
use crate::engine::wasi::cli::{I32Exit, TerminalInput, TerminalOutput};
use crate::engine::wasi::io::{InputStream, OutputStream};
use crate::Ctx;

impl terminal_input::Host for Ctx {}
impl terminal_output::Host for Ctx {}

impl terminal_input::HostTerminalInput for Ctx {
    fn drop(&mut self, rep: Resource<TerminalInput>) -> wasmtime::Result<()> {
        self.table
            .delete(rep)
            .context("failed to delete input resource from table")?;
        Ok(())
    }
}

impl terminal_output::HostTerminalOutput for Ctx {
    fn drop(&mut self, rep: Resource<TerminalOutput>) -> wasmtime::Result<()> {
        self.table
            .delete(rep)
            .context("failed to delete output resource from table")?;
        Ok(())
    }
}

impl terminal_stdin::Host for Ctx {
    fn get_terminal_stdin(&mut self) -> wasmtime::Result<Option<Resource<TerminalInput>>> {
        Ok(None)
    }
}

impl terminal_stdout::Host for Ctx {
    fn get_terminal_stdout(&mut self) -> wasmtime::Result<Option<Resource<TerminalOutput>>> {
        if self.stdout.is_terminal() {
            let fd = self
                .table
                .push(TerminalOutput)
                .context("failed to push terminal resource to table")?;
            Ok(Some(fd))
        } else {
            Ok(None)
        }
    }
}

impl terminal_stderr::Host for Ctx {
    fn get_terminal_stderr(&mut self) -> wasmtime::Result<Option<Resource<TerminalOutput>>> {
        if self.stderr.is_terminal() {
            let fd = self
                .table
                .push(TerminalOutput)
                .context("failed to push terminal resource to table")?;
            Ok(Some(fd))
        } else {
            Ok(None)
        }
    }
}

impl stdin::Host for Ctx {
    fn get_stdin(&mut self) -> wasmtime::Result<Resource<InputStream>> {
        let stream = match &self.stdin {
            InputStream::Empty => InputStream::Empty,
            InputStream::TcpStream(stream) => InputStream::TcpStream(Arc::clone(stream)),
            InputStream::UdpSocket(socket) => InputStream::UdpSocket(Arc::clone(socket)),
            InputStream::Http(stream) => InputStream::Http(Arc::clone(stream)),
        };
        let stream = self
            .table
            .push(stream)
            .context("failed to push input stream resource to table")?;
        Ok(stream)
    }
}

fn get_output_stream(stream: &OutputStream) -> wasmtime::Result<OutputStream> {
    match stream {
        OutputStream::Discard => Ok(OutputStream::Discard),
        OutputStream::Stdout(..) => Ok(OutputStream::Stdout(std::io::stdout())),
        OutputStream::Stderr(..) => Ok(OutputStream::Stderr(std::io::stderr())),
        OutputStream::TcpStream(stream) => Ok(OutputStream::TcpStream(Arc::clone(stream))),
        OutputStream::UdpSocket(socket) => Ok(OutputStream::UdpSocket(Arc::clone(socket))),
        OutputStream::Limited { budget, stream } => {
            let stream = get_output_stream(stream)?;
            Ok(OutputStream::Limited {
                budget: *budget,
                stream: Box::new(stream),
            })
        }
        OutputStream::HttpPending(..) => bail!("invalid output stream state"),
        OutputStream::HttpWriting(tx) => Ok(OutputStream::HttpWriting(tx.clone())),
        OutputStream::Tracing(span) => Ok(OutputStream::Tracing(span.clone())),
    }
}

impl stdout::Host for Ctx {
    fn get_stdout(&mut self) -> wasmtime::Result<Resource<OutputStream>> {
        let stream = get_output_stream(&self.stdout)?;
        let stream = self
            .table
            .push(stream)
            .context("failed to push output stream resource to table")?;
        Ok(stream)
    }
}

impl stderr::Host for Ctx {
    fn get_stderr(&mut self) -> wasmtime::Result<Resource<OutputStream>> {
        let stream = get_output_stream(&self.stderr)?;
        let stream = self
            .table
            .push(stream)
            .context("failed to push output stream resource to table")?;
        Ok(stream)
    }
}

impl environment::Host for Ctx {
    fn get_environment(&mut self) -> wasmtime::Result<Vec<(String, String)>> {
        Ok(self.environment.clone())
    }

    fn get_arguments(&mut self) -> wasmtime::Result<Vec<String>> {
        Ok(self.arguments.clone())
    }

    fn initial_cwd(&mut self) -> wasmtime::Result<Option<String>> {
        Ok(self.initial_cwd.clone())
    }
}

impl exit::Host for Ctx {
    fn exit(&mut self, status: Result<(), ()>) -> wasmtime::Result<()> {
        let status = match status {
            Ok(()) => 0,
            Err(()) => 1,
        };
        Err(anyhow!(I32Exit(status)))
    }

    fn exit_with_code(&mut self, status_code: u8) -> wasmtime::Result<()> {
        Err(anyhow!(I32Exit(status_code.into())))
    }
}
