//! Bounded loopback IPC with one-time authentication and exact process identity.
//! No shell commands, credentials or arbitrary executable paths are messages.
use super::process::{ProcessFence, ProcessTicket};
use rela_protocol::AppError;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::Path,
    thread,
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 8192;
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Controller,
    Candidate,
    Core,
    InstallerPre,
    InstallerPost,
    InstallerRecovery,
    InstallerLauncher,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub schema_version: u32,
    pub id: String,
    pub port: u16,
    token: String,
    pub role: Role,
}
impl Endpoint {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != 1 || self.port == 0 {
            return Err(protocol_error());
        }
        validate_id(&self.id)?;
        validate_id(&self.token)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    endpoint: Endpoint,
    process: ProcessTicket,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Accepted {
    schema_version: u32,
    id: String,
}

pub struct Listener {
    listener: TcpListener,
    endpoint: Endpoint,
}
impl Listener {
    pub fn bind(id: &str, role: Role) -> Result<Self, AppError> {
        validate_id(id)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| protocol_error())?;
        listener
            .set_nonblocking(true)
            .map_err(|_| protocol_error())?;
        let endpoint = Endpoint {
            schema_version: 1,
            id: id.into(),
            port: listener.local_addr().map_err(|_| protocol_error())?.port(),
            token: random_id()?,
            role,
        };
        Ok(Self { listener, endpoint })
    }
    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }
    pub fn accept(
        &self,
        expected_pid: u32,
        executable: &Path,
        timeout: Duration,
        exited: impl FnMut() -> bool,
    ) -> Result<Channel, AppError> {
        self.accept_verified(timeout, exited, |ticket| {
            if ticket.pid != expected_pid {
                return Err(protocol_error());
            }
            let process = ProcessFence::open(ticket, executable)?;
            if process.has_exited()? {
                return Err(protocol_error());
            }
            Ok(())
        })
        .map(|(channel, _)| channel)
    }

    /// The signed NSIS installer chooses the hook process id. After the same
    /// one-time token check, its caller must verify the process's real signed
    /// image and installer parent before any privileged protocol is dispatched.
    pub(crate) fn accept_verified(
        &self,
        timeout: Duration,
        mut exited: impl FnMut() -> bool,
        mut verify_process: impl FnMut(&ProcessTicket) -> Result<(), AppError>,
    ) -> Result<(Channel, ProcessTicket), AppError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline && !exited() {
            match self.listener.accept() {
                Ok((stream, address)) if address.ip().is_loopback() => {
                    let mut channel = Channel::new(stream, Duration::from_secs(1))?;
                    let hello = channel.receive::<Hello>();
                    if let Ok(hello) = hello {
                        if hello.endpoint.validate().is_ok()
                            && hello.endpoint.id == self.endpoint.id
                            && hello.endpoint.role == self.endpoint.role
                            && hello.endpoint.port == self.endpoint.port
                            && token_matches(&hello.endpoint.token, &self.endpoint.token)
                            && verify_process(&hello.process).is_ok()
                        {
                            channel.send(&Accepted {
                                schema_version: 1,
                                id: self.endpoint.id.clone(),
                            })?;
                            channel.timeout(Duration::from_secs(120))?;
                            return Ok((channel, hello.process));
                        }
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return Err(protocol_error()),
            }
        }
        Err(protocol_error())
    }
}

pub struct Channel {
    stream: TcpStream,
}
impl Channel {
    fn new(stream: TcpStream, timeout: Duration) -> Result<Self, AppError> {
        let result = Self { stream };
        // Windows accepted sockets inherit nonblocking mode from the listener.
        // Handshake/message I/O uses explicit bounded blocking timeouts.
        result
            .stream
            .set_nonblocking(false)
            .map_err(|_| protocol_error())?;
        result
            .stream
            .set_nodelay(true)
            .map_err(|_| protocol_error())?;
        result.timeout(timeout)?;
        Ok(result)
    }
    pub fn connect(endpoint: &Endpoint) -> Result<Self, AppError> {
        endpoint.validate()?;
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(10))
            .map_err(|_| protocol_error())?;
        let mut channel = Self::new(stream, Duration::from_secs(10))?;
        let executable = std::env::current_exe().map_err(|_| protocol_error())?;
        channel.send(&Hello {
            endpoint: endpoint.clone(),
            process: ProcessFence::capture(std::process::id(), &executable)?,
        })?;
        let accepted: Accepted = channel.receive()?;
        if accepted.schema_version != 1 || accepted.id != endpoint.id {
            return Err(protocol_error());
        }
        channel.timeout(Duration::from_secs(120))?;
        Ok(channel)
    }
    pub fn timeout(&self, timeout: Duration) -> Result<(), AppError> {
        self.stream
            .set_read_timeout(Some(timeout))
            .and_then(|_| {
                self.stream
                    .set_write_timeout(Some(timeout.min(Duration::from_secs(10))))
            })
            .map_err(|_| protocol_error())
    }
    pub fn send(&mut self, value: &impl Serialize) -> Result<(), AppError> {
        let bytes = serde_json::to_vec(value).map_err(|_| protocol_error())?;
        if bytes.is_empty() || bytes.len() > MAX_FRAME {
            return Err(protocol_error());
        }
        self.stream
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .and_then(|_| self.stream.write_all(&bytes))
            .map_err(|_| protocol_error())
    }
    pub fn receive<T: DeserializeOwned>(&mut self) -> Result<T, AppError> {
        let mut header = [0u8; 4];
        self.stream
            .read_exact(&mut header)
            .map_err(|_| protocol_error())?;
        let size = u32::from_le_bytes(header) as usize;
        if size == 0 || size > MAX_FRAME {
            return Err(protocol_error());
        }
        let mut bytes = vec![0; size];
        self.stream
            .read_exact(&mut bytes)
            .map_err(|_| protocol_error())?;
        serde_json::from_slice(&bytes).map_err(|_| protocol_error())
    }
}
fn token_matches(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |different, (l, r)| different | (l ^ r))
            == 0
}
pub fn random_id() -> Result<String, AppError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| protocol_error())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
pub fn validate_id(id: &str) -> Result<(), AppError> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(protocol_error());
    }
    Ok(())
}
pub fn protocol_error() -> AppError {
    AppError::new(
        "update_helper_unavailable",
        "更新助手未完成通信，已保留恢复状态。请重试更新或恢复操作。",
    )
}

#[cfg(test)]
mod tests;
