//! `comms` — terminal client. Thin shell over the application core.

mod app;
mod input;
mod keys;
mod render;
mod theme;
mod ui;

#[cfg(test)]
mod tests;

use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use comms_app::AppHandle;
use comms_bootstrap::{Paths, Settings};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream};
use futures::StreamExt;
use tokio::sync::broadcast::error::RecvError;

use crate::app::App;
use crate::keys::KeyMap;

const HELP: &str = "\
comms — a fast terminal client for Microsoft Teams, Signal and Slack

USAGE:
    comms                     start the client
    comms --demo              try the interface with fake data (no account needed)
    comms --demo-slack        the same, with the Slack workspace layout and threads
    comms login               sign in with your browser
    comms login --device-code sign in with a code (for SSH / no browser)
    comms signal-link         link this computer to your Signal account (needs signal-cli)
    comms logout [--wipe-cache]
                              forget the stored sign-in (and delete cached messages)
    comms paths               show where config, cache and logs live
    comms --version | --help

Teams: see docs/setup-entra.md.  Signal: see docs/signal.md (provider = signal).
Slack: see docs/slack.md (provider = slack, token in COMMS_SLACK_TOKEN).
Press ? inside the client for key bindings.";

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    match args.first().map(String::as_str) {
        Some("--help" | "-h" | "help") => {
            println!("{HELP}");
            return Ok(());
        }
        Some("--version" | "-V") => {
            println!("comms {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("paths") => {
            let p = Paths::discover()?;
            println!(
                "config: {}\ncache:  {}\nlog:    {}",
                p.config_file.display(),
                p.database.display(),
                p.log_file.display()
            );
            return Ok(());
        }
        Some("login") => {
            let settings = Settings::load()?;
            comms_bootstrap::login_command(&settings, has("--device-code"), |s| println!("{s}"))
                .await?;
            println!("Signed in. Start the client with `comms`.");
            return Ok(());
        }
        Some("signal-link") => {
            let settings = Settings::load()?;
            println!("Linking to your Signal account with signal-cli...");
            comms_bootstrap::signal_link(&settings, |s| println!("{s}")).await?;
            println!(
                "\nLinked. Now set your number in {}:\n\n    provider = \"signal\"\n    [signal]\n    account = \"+36...\"\n\nthen start `comms`.",
                Paths::discover()?.config_file.display()
            );
            return Ok(());
        }
        Some("logout") => {
            let settings = Settings::load()?;
            for line in comms_bootstrap::logout(&settings, has("--wipe-cache")).await? {
                println!("{line}");
            }
            return Ok(());
        }
        Some("--demo" | "--demo-slack") | None => {}
        Some(other) => bail!("unknown argument `{other}`\n\n{HELP}"),
    }

    let settings = Settings::load()?;
    let keymap = KeyMap::new(&settings.keys)
        .map_err(|e| anyhow::anyhow!("invalid key configuration: {e}"))?;
    init_logging()?;

    let handle: AppHandle = if has("--demo-slack") {
        comms_bootstrap::start_demo_slack().await?
    } else if has("--demo") {
        comms_bootstrap::start_demo().await?
    } else {
        comms_bootstrap::start(&settings, |s| println!("{s}")).await?
    };

    let mut app = App::new(handle, keymap).await;
    let mut terminal = ratatui::init();
    // Ask the terminal which image protocol it speaks (sixel, kitty, ...).
    // Must happen before the key event stream starts reading stdin.
    app.picker = Some(
        ratatui_image::picker::Picker::from_query_stdio()
            .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks()),
    );
    // The workspace name and icon arrive whenever the download finishes.
    let (ws_tx, ws_rx) = tokio::sync::mpsc::unbounded_channel();
    if !has("--demo") && !has("--demo-slack") {
        let settings = settings.clone();
        tokio::spawn(async move {
            if let Some(info) = comms_bootstrap::workspace_info(&settings).await {
                let _ = ws_tx.send(info);
            }
        });
    }
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste);
    let result = event_loop(&mut terminal, app, ws_rx).await;
    let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

/// Logs go to a private file: the terminal belongs to the UI. Message
/// contents and tokens are never logged.
fn init_logging() -> Result<()> {
    let paths = Paths::discover()?;
    if let Some(dir) = paths.log_file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts
        .open(&paths.log_file)
        .with_context(|| format!("opening {}", paths.log_file.display()))?;
    let filter = tracing_subscriber::EnvFilter::try_from_env("COMMS_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .init();
    Ok(())
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    mut app: App,
    mut workspace: tokio::sync::mpsc::UnboundedReceiver<comms_bootstrap::WorkspaceInfo>,
) -> Result<()> {
    let mut keys = EventStream::new();
    let mut events = app.handle.subscribe();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    terminal.draw(|f| ui::draw(f, &mut app))?;

    while !app.quit {
        let mut dirty = false;
        tokio::select! {
            ev = keys.next() => match ev {
                Some(Ok(Event::Key(k))) => { app.on_key(k).await; dirty = true; }
                Some(Ok(Event::Paste(s))) => { app.on_paste(&s).await; dirty = true; }
                Some(Ok(Event::Resize(..))) => dirty = true,
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => break,
            },
            ev = events.recv() => match ev {
                Ok(ev) => { app.on_event(ev).await; dirty = true; }
                Err(RecvError::Lagged(_)) => {
                    app.refresh_conversations().await;
                    app.refresh_messages().await;
                    dirty = true;
                }
                Err(RecvError::Closed) => break,
            },
            Some(info) = workspace.recv() => { app.set_workspace(info); dirty = true; }
            _ = tick.tick() => dirty = app.on_tick(),
        }
        if dirty {
            terminal.draw(|f| ui::draw(f, &mut app))?;
        }
    }
    let _ = std::io::stdout().flush();
    Ok(())
}
