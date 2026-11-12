//! The Signal backend (via `signal-cli`). Compiled with the `signal` feature.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use comms_app::AppHandle;
use comms_provider_signal::{SignalConfig, SignalProvider};
use comms_storage::Store;

use crate::{Paths, Settings, core_config};

/// A valid-looking international phone number (`+` and 7 to 15 digits).
pub fn valid_signal_account(s: &str) -> bool {
    let digits = s.strip_prefix('+').unwrap_or("");
    (7..=15).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The Signal backend. Needs `signal-cli` with the account already linked
/// (see [`signal_link`]).
pub async fn start_signal(settings: &Settings) -> Result<AppHandle> {
    let Some(account) = settings.signal.account.clone() else {
        bail!(
            "no Signal account configured.\n\
             Set `account = \"+36...\"` in the [signal] section of {} \
             (or COMMS_SIGNAL_ACCOUNT), and link it first with `comms signal-link`.",
            Paths::discover()?.config_file.display()
        );
    };
    if !valid_signal_account(&account) {
        bail!("signal.account must look like +36201234567 (international format), got `{account}`");
    }
    let paths = Paths::discover()?;
    let store = Store::open(&paths.database)
        .await
        .context("opening the local database")?;
    let provider = SignalProvider::connect(SignalConfig {
        account,
        cli_path: settings.signal.cli_path.clone(),
        inbox_file: Some(paths.signal_inbox),
    })
    .await?;
    Ok(AppHandle::start(Arc::new(provider), store, core_config(settings)).await?)
}

/// Link this computer to your Signal account as a secondary device.
pub async fn signal_link(settings: &Settings, say: impl Fn(&str)) -> Result<()> {
    comms_provider_signal::link(&settings.signal.cli_path, "comms", say).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_account_validation() {
        for ok in ["+36201234567", "+1234567"] {
            assert!(valid_signal_account(ok), "{ok}");
        }
        for bad in [
            "",
            "36201234567",
            "+",
            "+12",
            "+36 20 123",
            "+36a01234567",
            "+1234567890123456",
        ] {
            assert!(!valid_signal_account(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn starting_signal_without_an_account_explains_what_to_do() {
        let err = start_signal(&Settings::default())
            .await
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("signal-link") && err.contains("account"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn starting_signal_with_a_malformed_account_is_rejected() {
        let mut s = Settings::default();
        s.signal.account = Some("0620 123".into());
        let err = start_signal(&s).await.map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("+36201234567"), "{err}");
    }
}
