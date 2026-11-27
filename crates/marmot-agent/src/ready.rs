//! Saying that the core is ready (D-178): it has opened the agent's account store, and its driver
//! has started, a built-in harness having answered its start or the socket listening for a
//! connector. The installer counts an install as done only then.
//!
//! - On Linux, the service is `Type=notify`: the core sends `READY=1`, systemd's notify protocol,
//!   as one datagram to the socket `NOTIFY_SOCKET` names, a path or, from an `@`, an abstract
//!   name. No library: std's `UnixDatagram` is all the protocol needs.
//! - launchd has no such protocol, so the macOS launcher names a file in
//!   `MARMOT_AGENT_READY_FILE`, which the core makes, and the installer waits for.
//!
//! A core started by hand has neither, and nothing is done.

use std::ffi::OsString;
use std::os::unix::net::UnixDatagram;

/// Says the core is ready, as its service manager asked in the environment.
pub fn tell() -> Result<(), String> {
    tell_to(
        std::env::var_os("NOTIFY_SOCKET"),
        std::env::var_os("MARMOT_AGENT_READY_FILE"),
    )
}

/// Says the core is ready to `notify`, a socket as `NOTIFY_SOCKET` names one, and by making the
/// file `file`; neither when not given.
pub fn tell_to(notify: Option<OsString>, file: Option<OsString>) -> Result<(), String> {
    // The file first: whoever hears the datagram finds it made.
    if let Some(file) = file.filter(|f| !f.is_empty()) {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(file)
            .map_err(|e| format!("the file that says the core is ready could not be made: {e}"))?;
    }
    if let Some(notify) = notify.filter(|n| !n.is_empty()) {
        send(&notify).map_err(|e| format!("systemd could not be told the core is ready: {e}"))?;
    }
    Ok(())
}

fn send(notify: &OsString) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let socket = UnixDatagram::unbound()?;
    let name = notify.as_bytes();
    if let Some(abstract_name) = name.strip_prefix(b"@") {
        return send_abstract(&socket, abstract_name);
    }
    if name.first() != Some(&b'/') {
        // systemd also names `vsock:` addresses, for a virtual machine's manager, never a user's.
        return Err(std::io::Error::other("not a socket's path"));
    }
    socket.send_to(b"READY=1", notify).map(|_| ())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn send_abstract(socket: &UnixDatagram, name: &[u8]) -> std::io::Result<()> {
    use std::os::linux::net::SocketAddrExt;
    let address = std::os::unix::net::SocketAddr::from_abstract_name(name)?;
    socket.send_to_addr(b"READY=1", &address).map(|_| ())
}

/// Abstract sockets are Linux's alone, and so is systemd.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn send_abstract(_socket: &UnixDatagram, _name: &[u8]) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "an abstract socket, which only Linux has",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn received(socket: &UnixDatagram) -> Vec<u8> {
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buffer = [0; 64];
        let n = socket.recv(&mut buffer).expect("a message");
        buffer[..n].to_vec()
    }

    #[test]
    fn ready_reaches_a_socket_named_by_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify");
        let socket = UnixDatagram::bind(&path).unwrap();
        tell_to(Some(path.into_os_string()), None).unwrap();
        assert_eq!(received(&socket), b"READY=1");
    }

    /// systemd names an abstract socket for a user's services where it can (`@` and its name).
    #[cfg(target_os = "linux")]
    #[test]
    fn ready_reaches_an_abstract_socket() {
        use std::os::linux::net::SocketAddrExt;
        let name = format!("marmot-agent-ready-test-{}", std::process::id());
        let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let socket = UnixDatagram::bind_addr(&address).unwrap();
        tell_to(Some(format!("@{name}").into()), None).unwrap();
        assert_eq!(received(&socket), b"READY=1");
    }

    #[test]
    fn the_ready_file_is_made_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("core.ready");
        tell_to(None, Some(file.clone().into_os_string())).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// A core started by hand, with neither, does nothing; an address it cannot use is an error,
    /// which the core reports, and systemd then counts as a failed start once it waited.
    #[test]
    fn nothing_is_said_without_a_service_manager_and_a_bad_address_fails() {
        assert_eq!(tell_to(None, None), Ok(()));
        assert_eq!(tell_to(Some("".into()), Some("".into())), Ok(()));
        assert!(tell_to(Some("vsock:2:1".into()), None).is_err());
        assert!(tell_to(Some("/nowhere/notify".into()), None).is_err());
    }
}
