//! Where the search API key lives: the system keyring, through the Secret
//! Service API (KWallet answers it on Plasma, GNOME Keyring elsewhere), with
//! `oo7`. Never the settings file, never a conversation, never a log.
//!
//! Without a keyring (the dev container, CI, a minimal session) there is no
//! key to keep: Settings says so and the key isn't saved anywhere. The rest
//! of the app talks to the `KeyStore` trait, so tests use `Memory`.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

/// The name the web search key is kept under.
pub const SEARCH_KEY: &str = "web-search-key";

/// The application attribute of every item Gates keeps.
const APPLICATION: &str = "net.eterneon.telamon.gates";

/// A locked keyring may ask the user for a password; that is waited for this
/// long.
const WAIT: Duration = Duration::from_secs(90);

/// A place to keep secrets. Calls block (a locked keyring asks the user):
/// call them from a worker, never the GUI thread.
pub trait KeyStore: Send + Sync {
    /// Ok when a keyring answers; else why not, as a sentence.
    fn check(&self) -> Result<(), String>;
    /// The secret kept under `name`; None when there is none.
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    /// Keeps `secret` under `name`, replacing one there.
    fn set(&self, name: &str, secret: &str) -> Result<(), String>;
    /// Forgets `name`; fine when there was none.
    fn remove(&self, name: &str) -> Result<(), String>;
}

/// The system keyring.
#[derive(Debug, Default, Clone, Copy)]
pub struct SecretService;

/// Runs `future` to its end on a runtime of its own, for at most `WAIT`.
fn block<T>(future: impl Future<Output = oo7::Result<T>>) -> Result<T, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("The system keyring can't be reached: {e}."))?;
    runtime
        .block_on(async { tokio::time::timeout(WAIT, future).await })
        .map_err(|_| "The system keyring didn't answer. Is it waiting to be unlocked?".to_string())?
        .map_err(|e| format!("The system keyring can't be used: {e}."))
}

fn attributes(name: &str) -> [(&str, &str); 2] {
    [("application", APPLICATION), ("name", name)]
}

impl KeyStore for SecretService {
    fn check(&self) -> Result<(), String> {
        // Reaching the service is enough: no wallet is opened, created or
        // unlocked, so nothing asks the user anything.
        block(async {
            oo7::dbus::Service::new()
                .await
                .map(|_| ())
                .map_err(oo7::Error::from)
        })
        .map_err(|detail| {
            // The bus's own words ("The name org.freedesktop.secrets was not
            // provided by any .service files") go to the log, not the user.
            log::debug!("keyring: {detail}");
            if detail.contains("didn't answer") {
                detail
            } else {
                NO_KEYRING.to_string()
            }
        })
    }

    fn get(&self, name: &str) -> Result<Option<String>, String> {
        block(async {
            let keyring = oo7::Keyring::new().await?;
            keyring.unlock().await?;
            let items = keyring.search_items(&attributes(name)).await?;
            let Some(item) = items.first() else {
                return Ok(None);
            };
            item.unlock().await?;
            let secret = item.secret().await?;
            Ok(Some(
                String::from_utf8_lossy(secret.as_bytes()).into_owned(),
            ))
        })
    }

    fn set(&self, name: &str, secret: &str) -> Result<(), String> {
        block(async {
            let keyring = oo7::Keyring::new().await?;
            keyring.unlock().await?;
            keyring
                .create_item(
                    "Telamon Gates: web search key",
                    &attributes(name),
                    oo7::Secret::text(secret),
                    true,
                )
                .await
        })
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        block(async {
            let keyring = oo7::Keyring::new().await?;
            keyring.unlock().await?;
            keyring.delete(&attributes(name)).await
        })
    }
}

/// Secrets in memory, for tests and demos. Nothing is written anywhere.
#[derive(Debug, Default)]
pub struct Memory {
    items: Mutex<HashMap<String, String>>,
}

impl KeyStore for Memory {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self
            .items
            .lock()
            .map_err(|_| "poisoned")?
            .get(name)
            .cloned())
    }

    fn set(&self, name: &str, secret: &str) -> Result<(), String> {
        self.items
            .lock()
            .map_err(|_| "poisoned")?
            .insert(name.to_string(), secret.to_string());
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        self.items.lock().map_err(|_| "poisoned")?.remove(name);
        Ok(())
    }
}

/// No keyring at all: every call says so. What the dev container and CI have.
#[derive(Debug, Default, Clone, Copy)]
pub struct Missing;

pub const NO_KEYRING: &str =
    "No system keyring is running (such as KWallet), so the key can't be kept safely.";

impl KeyStore for Missing {
    fn check(&self) -> Result<(), String> {
        Err(NO_KEYRING.into())
    }
    fn get(&self, _: &str) -> Result<Option<String>, String> {
        Err(NO_KEYRING.into())
    }
    fn set(&self, _: &str, _: &str) -> Result<(), String> {
        Err(NO_KEYRING.into())
    }
    fn remove(&self, _: &str) -> Result<(), String> {
        Err(NO_KEYRING.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_store_keeps_replaces_and_forgets() {
        let store = Memory::default();
        assert_eq!(store.get(SEARCH_KEY), Ok(None));
        store.set(SEARCH_KEY, "first").unwrap();
        store.set(SEARCH_KEY, "second").unwrap();
        assert_eq!(store.get(SEARCH_KEY), Ok(Some("second".into())));
        store.remove(SEARCH_KEY).unwrap();
        store.remove(SEARCH_KEY).unwrap();
        assert_eq!(store.get(SEARCH_KEY), Ok(None));
    }

    #[test]
    fn no_keyring_means_no_key() {
        let store: &dyn KeyStore = &Missing;
        assert!(store.check().unwrap_err().contains("keyring"));
        // Saving says no, and nothing is kept anywhere.
        assert!(store.set(SEARCH_KEY, "test-key-not-real").is_err());
        assert!(store.get(SEARCH_KEY).is_err());
    }

    /// Where no session bus or Secret Service runs (the dev container, CI),
    /// the real store reports it instead of hanging or panicking. Where
    /// there is one, this only checks that the call comes back.
    #[test]
    fn the_system_keyring_answers_or_says_why_not() {
        let started = std::time::Instant::now();
        match SecretService.check() {
            Ok(()) => {}
            Err(e) => assert!(e.contains("keyring"), "{e}"),
        }
        assert!(started.elapsed() < Duration::from_secs(30));
    }
}
