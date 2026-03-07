//! Generic typed IPC over pipes with fork.
//!
//! Provides `TypedWriter`/`TypedReader` for length-prefixed bincode messages,
//! `ChildProcess` for child lifetime management, and `fork_with_channels`
//! to spawn a child with bidirectional typed channels.

use std::fs::File;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::os::fd::{FromRawFd, IntoRawFd};

use anyhow::{Context, Result};
use nix::libc;
use nix::sys::signal;
use nix::unistd::{ForkResult, Pid, fork, pipe};
use serde::{Serialize, de::DeserializeOwned};

/// Typed writer: sends length-prefixed bincode messages over a pipe.
pub struct TypedWriter<T> {
    file: File,
    _phantom: PhantomData<T>,
}

/// Typed reader: receives length-prefixed bincode messages from a pipe.
pub struct TypedReader<T> {
    file: File,
    _phantom: PhantomData<T>,
}

impl<T: Serialize> TypedWriter<T> {
    fn new(file: File) -> Self {
        Self {
            file,
            _phantom: PhantomData,
        }
    }

    /// Send a message. Wire format: [u32 LE length][bincode payload].
    pub fn send(&mut self, msg: &T) -> Result<()> {
        let payload = bincode::serde::encode_to_vec(msg, bincode::config::standard())
            .context("bincode encode")?;
        let len = payload.len() as u32;
        self.file
            .write_all(&len.to_le_bytes())
            .context("write length")?;
        self.file.write_all(&payload).context("write payload")?;
        Ok(())
    }
}

impl<T: DeserializeOwned> TypedReader<T> {
    fn new(file: File) -> Self {
        Self {
            file,
            _phantom: PhantomData,
        }
    }

    /// Receive a message. Blocks until a complete message is available.
    pub fn recv(&mut self) -> Result<T> {
        let mut len_buf = [0u8; 4];
        self.file
            .read_exact(&mut len_buf)
            .context("read length (child may have exited)")?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut payload = vec![0u8; len];
        self.file.read_exact(&mut payload).context("read payload")?;
        let (msg, _) = bincode::serde::decode_from_slice(&payload, bincode::config::standard())
            .context("bincode decode")?;
        Ok(msg)
    }
}

/// Handle to a forked child process. Sends SIGKILL on drop.
pub struct ChildProcess {
    pid: Pid,
    alive: bool,
}

impl ChildProcess {
    /// Wait for the child to exit and return its status code.
    pub fn wait(&mut self) -> Result<i32> {
        use nix::sys::wait::{WaitStatus, waitpid};
        self.alive = false;
        match waitpid(self.pid, None).context("waitpid")? {
            WaitStatus::Exited(_, code) => Ok(code),
            WaitStatus::Signaled(_, sig, _) => Ok(128 + sig as i32),
            other => anyhow::bail!("unexpected wait status: {:?}", other),
        }
    }

    /// Send SIGKILL to the child (best-effort).
    pub fn kill(&mut self) {
        if self.alive {
            self.alive = false;
            let _ = signal::kill(self.pid, signal::Signal::SIGKILL);
            let _ = nix::sys::wait::waitpid(self.pid, None);
        }
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Fork a child process with two typed channels (parent→child, child→parent).
///
/// The child executes `child_fn` with a reader (for requests) and writer (for responses).
/// The parent receives a writer (to send requests), reader (to receive responses),
/// and a `ChildProcess` handle.
///
/// # Safety
/// Uses `fork()` which is unsafe in multi-threaded programs. Call before spawning threads.
pub fn fork_with_channels<Req, Resp, F>(
    child_fn: F,
) -> Result<(TypedWriter<Req>, TypedReader<Resp>, ChildProcess)>
where
    Req: Serialize + DeserializeOwned,
    Resp: Serialize + DeserializeOwned,
    F: FnOnce(TypedReader<Req>, TypedWriter<Resp>),
{
    // parent→child pipe (OwnedFd)
    let (p2c_read, p2c_write) = pipe().context("pipe p2c")?;
    // child→parent pipe
    let (c2p_read, c2p_write) = pipe().context("pipe c2p")?;

    // SAFETY: fork() is called before any worker threads are spawned.
    match unsafe { fork() }.context("fork")? {
        ForkResult::Child => {
            // Close parent ends by dropping them
            drop(p2c_write);
            drop(c2p_read);

            let reader = TypedReader::new(unsafe { File::from_raw_fd(p2c_read.into_raw_fd()) });
            let writer = TypedWriter::new(unsafe { File::from_raw_fd(c2p_write.into_raw_fd()) });

            child_fn(reader, writer);
            // Use _exit(2) instead of std::process::exit() to avoid running
            // atexit handlers inherited from the parent (e.g. test harness
            // thread joins that would deadlock in the forked child).
            unsafe { libc::_exit(0) }
        }
        ForkResult::Parent { child } => {
            // Close child ends by dropping them
            drop(p2c_read);
            drop(c2p_write);

            let writer = TypedWriter::new(unsafe { File::from_raw_fd(p2c_write.into_raw_fd()) });
            let reader = TypedReader::new(unsafe { File::from_raw_fd(c2p_read.into_raw_fd()) });

            Ok((
                writer,
                reader,
                ChildProcess {
                    pid: child,
                    alive: true,
                },
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_send_recv() {
        let (read_fd, write_fd) = pipe().unwrap();

        let mut writer: TypedWriter<String> =
            TypedWriter::new(unsafe { File::from_raw_fd(write_fd.into_raw_fd()) });
        let mut reader: TypedReader<String> =
            TypedReader::new(unsafe { File::from_raw_fd(read_fd.into_raw_fd()) });

        writer.send(&"hello".to_string()).unwrap();
        writer.send(&"world".to_string()).unwrap();

        assert_eq!(reader.recv().unwrap(), "hello");
        assert_eq!(reader.recv().unwrap(), "world");
    }

    #[test]
    fn roundtrip_struct() {
        use serde::Deserialize;

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Msg {
            id: u32,
            data: Vec<u8>,
        }

        let (read_fd, write_fd) = pipe().unwrap();

        let mut writer: TypedWriter<Msg> =
            TypedWriter::new(unsafe { File::from_raw_fd(write_fd.into_raw_fd()) });
        let mut reader: TypedReader<Msg> =
            TypedReader::new(unsafe { File::from_raw_fd(read_fd.into_raw_fd()) });

        let msg = Msg {
            id: 42,
            data: vec![1, 2, 3],
        };
        writer.send(&msg).unwrap();
        let received = reader.recv().unwrap();
        assert_eq!(received, msg);
    }

    #[test]
    fn fork_channels_request_response() {
        let (mut writer, mut reader, mut child) =
            fork_with_channels::<u32, u64, _>(|mut req_reader, mut resp_writer| {
                let req: u32 = req_reader.recv().unwrap();
                resp_writer.send(&(req as u64 * 2)).unwrap();
            })
            .unwrap();

        writer.send(&21u32).unwrap();
        let resp = reader.recv().unwrap();
        assert_eq!(resp, 42u64);
        assert_eq!(child.wait().unwrap(), 0);
    }

    #[test]
    fn fork_channels_multiple_messages() {
        let (mut writer, mut reader, mut child) =
            fork_with_channels::<String, usize, _>(|mut req_reader, mut resp_writer| {
                loop {
                    match req_reader.recv() {
                        Ok(s) => resp_writer.send(&s.len()).unwrap(),
                        Err(_) => break,
                    }
                }
            })
            .unwrap();

        let inputs = ["hello", "world!", "foo"];
        for s in &inputs {
            writer.send(&s.to_string()).unwrap();
        }
        // Drop writer to close the pipe, signalling EOF to the child.
        drop(writer);

        for s in &inputs {
            assert_eq!(reader.recv().unwrap(), s.len());
        }
        assert_eq!(child.wait().unwrap(), 0);
    }

    #[test]
    fn fork_channels_child_exit_code() {
        let (_, _, mut child) = fork_with_channels::<u32, u32, _>(|_req_reader, _resp_writer| {
            // Return immediately without exchanging messages.
        })
        .unwrap();

        assert_eq!(child.wait().unwrap(), 0);
    }

    #[test]
    fn fork_channels_kill_on_drop() {
        let (_, _, child) = fork_with_channels::<u32, u32, _>(|_req_reader, _resp_writer| {
            // Spin forever — relies on Drop sending SIGKILL.
            loop {
                std::hint::spin_loop();
            }
        })
        .unwrap();

        // Dropping child sends SIGKILL and waits; if SIGKILL wasn't sent this
        // test would hang forever.
        drop(child);
    }
}
