//! `comms` — terminal client. Thin shell over the application core.

mod app;
mod input;
mod keys;
mod render;
mod theme;
mod ui;

#[cfg(test)]
mod tests;

use std::io::{IsTerminal, Write};
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
    comms slack-login         sign in to Slack: through the browser (needs [slack] client_id), or\n                              paste your user token (--token forces this); saved to the config
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
        Some("slack-login") => {
            let settings = Settings::load()?;
            slack_setup(&settings, has("--token")).await?;
            println!("Start the client with `comms`.");
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

    let mut settings = Settings::load()?;
    // First start with Slack and no token yet: offer to set it up right here.
    if !has("--demo")
        && !has("--demo-slack")
        && settings.provider == comms_bootstrap::Backend::Slack
        && settings.slack.token.is_none()
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
    {
        slack_setup(&settings, false).await?;
        settings = Settings::load()?;
    }
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

    let mut app = App::new(handle, keymap.clone()).await;
    let mut terminal = ratatui::init();
    // Ask the terminal which image protocol it speaks (sixel, kitty, ...).
    // Must happen before the key event stream starts reading stdin.
    app.picker = Some(
        ratatui_image::picker::Picker::from_query_stdio()
            .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks()),
    );
    // The workspace name and icon arrive whenever the download finishes.
    let (ws_tx, ws_rx) = tokio::sync::mpsc::unbounded_channel();
    let demo = has("--demo") || has("--demo-slack");
    if !demo {
        app.apps = comms_bootstrap::compiled_backends()
            .into_iter()
            .map(|b| b.name().to_owned())
            .collect();
        app.app_name = settings.provider.name().to_owned();
        spawn_workspace_fetch(&settings, 0, &ws_tx);
    }
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste);
    let result = event_loop(&mut terminal, app, ws_rx, ws_tx, settings, keymap).await;
    let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

/// Give Comms a Slack token: through the browser when a client ID is
/// configured (and `token_only` is not set), otherwise by pasting it.
async fn slack_setup(settings: &Settings, token_only: bool) -> Result<()> {
    let who = if settings.slack.client_id.is_some() && !token_only {
        comms_bootstrap::slack_login_browser(settings, |s| println!("{s}")).await?
    } else {
        println!(
            "Slack needs your User OAuth Token (starts with xoxp-).\n\
             Create the app from the manifest in docs/slack.md (api.slack.com/apps), install it,\n\
             then copy the token from OAuth & Permissions. Not the xoxb- or xapp- ones.\n\
             (With a shared app's Client ID in COMMS_SLACK_CLIENT_ID, `comms slack-login` signs in\n\
             through the browser instead.)\n"
        );
        let token = read_secret("Token (input is hidden, Esc cancels): ")?;
        comms_bootstrap::slack_login(&token).await?
    };
    println!(
        "Signed in to Slack as {who}. Saved to {} (readable only by you).",
        Paths::discover()?.config_file.display()
    );
    Ok(())
}

/// Read a line from the terminal without echoing it. Pasting works.
fn read_secret(prompt: &str) -> Result<String> {
    use crossterm::event::{self, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    fn read_loop() -> Result<String> {
        let mut buf = String::new();
        loop {
            match event::read()? {
                Event::Paste(s) => buf.push_str(s.trim()),
                Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                    KeyCode::Enter => return Ok(buf),
                    KeyCode::Esc => bail!("cancelled"),
                    KeyCode::Char('c' | 'd') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        bail!("cancelled")
                    }
                    KeyCode::Backspace => {
                        buf.pop();
                    }
                    KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => buf.push(c),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    print!("{prompt}");
    std::io::stdout().flush()?;
    enable_raw_mode()?;
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste);
    let result = read_loop();
    let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    let _ = disable_raw_mode();
    println!();
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

type WorkspaceMsg = (u64, comms_bootstrap::WorkspaceInfo);

/// Fetch the workspace name and icon in the background. `generation` tells
/// the loop which connection the answer belongs to (`:switch` bumps it).
fn spawn_workspace_fetch(
    settings: &Settings,
    generation: u64,
    tx: &tokio::sync::mpsc::UnboundedSender<WorkspaceMsg>,
) {
    let settings = settings.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        if let Some(info) = comms_bootstrap::workspace_info(&settings).await {
            let _ = tx.send((generation, info));
        }
    });
}

/// `:switch <app>`: start the other backend and replace the whole UI state
/// with a fresh one. Dropping the old handle stops its engine (and signal-cli).
async fn switch_app(app: &mut App, name: &str, settings: &Settings, keymap: &KeyMap) -> Result<()> {
    let backend = comms_bootstrap::Backend::from_name(name).context("unknown app")?;
    let mut next_settings = settings.clone();
    next_settings.provider = backend;
    // Sign-in prompts would corrupt the screen; a missing sign-in is an error here.
    let handle = comms_bootstrap::start(&next_settings, |_| {}).await?;
    let mut next = App::new(handle, keymap.clone()).await;
    next.picker = app.picker.take();
    next.apps = std::mem::take(&mut app.apps);
    next.app_name = backend.name().to_owned();
    *app = next;
    Ok(())
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    mut app: App,
    mut workspace: tokio::sync::mpsc::UnboundedReceiver<WorkspaceMsg>,
    workspace_tx: tokio::sync::mpsc::UnboundedSender<WorkspaceMsg>,
    settings: Settings,
    keymap: KeyMap,
) -> Result<()> {
    let mut generation = 0u64;
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
            Some((g, info)) = workspace.recv() => {
                if g == generation { app.set_workspace(info); dirty = true; }
            }
            _ = tick.tick() => dirty = app.on_tick(),
        }
        if let Some(name) = app.switch_to.take() {
            app.toast(format!("Switching to {name}…"), false);
            terminal.draw(|f| ui::draw(f, &mut app))?;
            match switch_app(&mut app, &name, &settings, &keymap).await {
                Ok(()) => {
                    generation += 1;
                    events = app.handle.subscribe();
                    let mut next = settings.clone();
                    next.provider =
                        comms_bootstrap::Backend::from_name(&name).unwrap_or(next.provider);
                    spawn_workspace_fetch(&next, generation, &workspace_tx);
                    app.toast(format!("Switched to {name}"), false);
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    app.toast(
                        format!(
                            "Could not switch to {name}: {}",
                            msg.lines().next().unwrap_or("")
                        ),
                        true,
                    );
                }
            }
            dirty = true;
        }
        if dirty {
            terminal.draw(|f| ui::draw(f, &mut app))?;
        }
    }
    let _ = std::io::stdout().flush();
    Ok(())
}
