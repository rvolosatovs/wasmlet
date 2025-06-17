use core::fmt;

use wasmtime::component::Linker;

use crate::engine::bindings::wasi::cli;
use crate::Ctx;

mod host;

/// Add all WASI interfaces from this module into the `linker` provided.
pub fn add_to_linker<T: Send>(
    linker: &mut Linker<T>,
    get: impl Fn(&mut T) -> &mut Ctx + Copy + Sync + Send + 'static,
) -> wasmtime::Result<()> {
    add_to_linker_with_options(linker, get, &cli::exit::LinkOptions::default())
}

/// Similar to [`add_to_linker`], but with the ability to enable unstable features.
pub fn add_to_linker_with_options<T: Send>(
    linker: &mut Linker<T>,
    get: impl Fn(&mut T) -> &mut Ctx + Copy + Sync + Send + 'static,
    exit_options: &cli::exit::LinkOptions,
) -> anyhow::Result<()> {
    cli::environment::add_to_linker(linker, get)?;
    cli::exit::add_to_linker(linker, exit_options, get)?;
    cli::stdin::add_to_linker(linker, get)?;
    cli::stdout::add_to_linker(linker, get)?;
    cli::stderr::add_to_linker(linker, get)?;
    cli::terminal_input::add_to_linker(linker, get)?;
    cli::terminal_output::add_to_linker(linker, get)?;
    cli::terminal_stdin::add_to_linker(linker, get)?;
    cli::terminal_stdout::add_to_linker(linker, get)?;
    cli::terminal_stderr::add_to_linker(linker, get)?;
    Ok(())
}

/// An error returned from the `proc_exit` host syscall.
///
/// Embedders can test if an error returned from wasm is this error, in which
/// case it may signal a non-fatal trap.
#[derive(Debug)]
pub struct I32Exit(pub i32);

impl fmt::Display for I32Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Exited with i32 exit status {}", self.0)
    }
}

impl std::error::Error for I32Exit {}

pub struct TerminalInput;
pub struct TerminalOutput;
