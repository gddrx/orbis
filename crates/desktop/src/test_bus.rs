//! A private bus for tests of the D-Bus services.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use zbus::{Connection, connection};

/// A session bus that starts no services on demand: with the system's
/// session configuration a call to a missing service would start the real
/// one (Dolphin for `org.freedesktop.FileManager1`) on the user's desktop.
const CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

/// A `dbus-daemon` of its own for one test; killed when dropped.
pub struct Bus {
    daemon: Child,
    config: PathBuf,
    pub address: String,
}

impl Bus {
    /// `None` when `dbus-daemon` is not installed.
    pub fn start() -> Option<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let config = std::env::temp_dir().join(format!(
            "orbis-test-bus-{}-{}.conf",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&config, CONFIG).ok()?;
        let mut daemon = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut line = String::new();
        BufReader::new(daemon.stdout.take()?)
            .read_line(&mut line)
            .ok()?;
        Some(Self {
            daemon,
            config,
            address: line.trim().to_owned(),
        })
    }

    pub async fn connect(&self) -> Connection {
        connection::Builder::address(self.address.as_str())
            .build()
            .await
            .unwrap()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_file(&self.config);
    }
}
