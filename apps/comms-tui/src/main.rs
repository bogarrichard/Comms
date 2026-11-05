//! `comms` — terminal client. Thin shell over the application core.

mod app;
mod input;
mod keys;
mod render;
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
comms — a fast terminal client for Microsoft Teams chats and channels

USAGE:
    comms                     start the client
    comms --demo              try the interface with fake data (no account needed)
    comms login               sign in with your browser
    comms login --device-code sign in with a code (for SSH / no browser)
    comms logout [--wipe-cache]
                              forget the stored sign-in (and delete cached messages)
    comms paths               show where config, cache and logs live
    comms --version | --help

Configuration: see docs/setup-entra.md. Press ? inside the client for key bindings.";

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
            let session = comms_bootstrap::make_session(&settings)?;
            comms_bootstrap::login(&session, has("--device-code"), |s| println!("{s}")).await?;
            println!("Signed in. Start the client with `comms`.");
            return Ok(());
        }
        Some("logout") => {
            let settings = Settings::load()?;
            for line in comms_bootstrap::logout(&settings, has("--wipe-cache")).await? {
                println!("{line}");
            }
            return Ok(());
        }
        Some("--demo") | None => {}
        Some(other) => bail!("unknown argument `{other}`\n\n{HELP}"),
    }

    let settings = Settings::load()?;
    let keymap = KeyMap::new(&settings.keys)
        .map_err(|e| anyhow::anyhow!("invalid key configuration: {e}"))?;
    init_logging()?;

    let handle: AppHandle = if has("--demo") {
        comms_bootstrap::start_demo().await?
    } else {
        let session = comms_bootstrap::make_session(&settings)?;
        if !session
            .is_signed_in()
            .await
            .map_err(comms_bootstrap::explain_auth_error)?
        {
            println!("You are not signed in yet.");
            comms_bootstrap::login(&session, false, |s| println!("{s}")).await?;
        }
        comms_bootstrap::start_graph(&settings, session).await?
    };

    let app = App::new(handle, keymap).await;
    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste);
    let result = event_loop(&mut terminal, app).await;
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

async fn event_loop(terminal: &mut ratatui::DefaultTerminal, mut app: App) -> Result<()> {
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
            _ = tick.tick() => dirty = app.on_tick(),
        }
        if dirty {
            terminal.draw(|f| ui::draw(f, &mut app))?;
        }
    }
    let _ = std::io::stdout().flush();
    Ok(())
}
