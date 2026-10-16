use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use discord_rich_presence::DiscordIpc;
use discord_rich_presence::error::Error;

/// Environment variables Discord reads, in order, to pick the folder for its socket.
const ROOT_VARIABLES: [&str; 4] = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"];
/// Where Discord puts its socket when none of the variables is set.
const FALLBACK_ROOT: &str = "/tmp";
/// Folders under the root where a sandboxed Discord build or client keeps its socket. The empty
/// one is a native install, Canary and PTB included.
const SOCKET_DIRS: [&str; 9] = [
    "",
    "app/com.discordapp.Discord",
    "app/com.discordapp.DiscordCanary",
    "app/dev.vencord.Vesktop",
    ".flatpak/com.discordapp.Discord/xdg-run",
    ".flatpak/com.discordapp.DiscordCanary/xdg-run",
    ".flatpak/dev.vencord.Vesktop/xdg-run",
    "snap.discord",
    "snap.discord-canary",
];
/// Discord numbers its socket from 0 and takes the first free slot, so a second running client
/// such as Canary next to stable lands on 1.
const SOCKET_SLOTS: u8 = 10;

/// A connection to Discord's IPC socket on Unix. It replaces the crate's client, which stops at
/// the first socket file it sees even when that file is stale, and does not know the Flatpak
/// Canary path.
pub(super) struct Socket {
    client_id: String,
    stream: Option<UnixStream>,
}

impl Socket {
    pub(super) fn new(client_id: &str) -> Self {
        Self {
            client_id: client_id.to_owned(),
            stream: None,
        }
    }
}

impl DiscordIpc for Socket {
    fn get_client_id(&self) -> &str {
        &self.client_id
    }

    /// Connects to the first candidate socket that accepts, so a file left by a client that has
    /// since quit is skipped rather than taken as the answer.
    fn connect_ipc(&mut self) -> Result<(), Error> {
        let stream = candidates()
            .find_map(|path| UnixStream::connect(path).ok())
            .ok_or(Error::IPCNotFound)?;
        self.stream = Some(stream);
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> Result<(), Error> {
        let stream = self.stream.as_mut().ok_or(Error::NotConnected)?;
        stream.write_all(data).map_err(Error::WriteError)
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<(), Error> {
        let stream = self.stream.as_mut().ok_or(Error::NotConnected)?;
        stream.read_exact(buffer).map_err(Error::ReadError)
    }

    fn close(&mut self) -> Result<(), Error> {
        self.stream = None;
        Ok(())
    }
}

/// Every path Discord's socket may have, most likely first.
fn candidates() -> impl Iterator<Item = PathBuf> {
    ROOT_VARIABLES
        .into_iter()
        .filter_map(env::var_os)
        .map(PathBuf::from)
        .chain([PathBuf::from(FALLBACK_ROOT)])
        .flat_map(|root| SOCKET_DIRS.map(|dir| root.join(dir)))
        .flat_map(|dir| (0..SOCKET_SLOTS).map(move |slot| dir.join(format!("discord-ipc-{slot}"))))
}
