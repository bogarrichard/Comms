//! Starting and linking `signal-cli`.

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::ConnectError;

pub struct Spawned {
    pub child: Child,
    pub stdout: ChildStdout,
    pub stdin: ChildStdin,
}

fn not_found(cli: &str) -> ConnectError {
    ConnectError(format!(
        "could not run `{cli}`. Install signal-cli (https://github.com/AsamK/signal-cli) \
         or set `cli_path` in the [signal] section of the config."
    ))
}

/// Start `signal-cli --account <account> jsonRpc`. The child is killed when dropped.
pub fn spawn(cli: &str, account: &str) -> Result<Spawned, ConnectError> {
    let mut child = Command::new(cli)
        .args(["--account", account, "jsonRpc"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // signal-cli logs numbers and errors to stderr; the terminal belongs to the UI.
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                not_found(cli)
            } else {
                ConnectError(format!("starting `{cli}`: {e}"))
            }
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ConnectError("no stdout from signal-cli".into()))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| ConnectError("no stdin for signal-cli".into()))?;
    Ok(Spawned {
        child,
        stdout,
        stdin,
    })
}

/// Link this machine as a secondary device (like Signal Desktop). Prints the
/// device-link address through `say`, and a QR code too when `qrencode` is installed.
pub async fn link(cli: &str, device_name: &str, say: impl Fn(&str)) -> Result<(), ConnectError> {
    let mut child = Command::new(cli)
        .args(["link", "-n", device_name])
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                not_found(cli)
            } else {
                ConnectError(format!("starting `{cli}`: {e}"))
            }
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ConnectError("no stdout from signal-cli".into()))?;
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.starts_with("sgnl://") || line.starts_with("tsdevice:") {
            if let Some(qr) = render_qr(line) {
                say("Scan this with Signal on your phone (Settings → Linked devices → +):");
                say(&qr);
            }
            say("Or open this address on the phone:");
        }
        if !line.is_empty() {
            say(line);
        }
    }
    let status = child
        .wait()
        .await
        .map_err(|e| ConnectError(format!("waiting for signal-cli: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(ConnectError(format!("signal-cli link failed ({status})")))
    }
}

/// A terminal QR code via `qrencode`, if that tool is installed.
fn render_qr(uri: &str) -> Option<String> {
    let out = std::process::Command::new("qrencode")
        .args(["-t", "ANSIUTF8", "-m", "1", "-o", "-", uri])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
