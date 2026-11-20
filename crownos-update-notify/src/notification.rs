use std::collections::HashMap;

use update_client::proto::ReleaseVersion;
use zbus::{Connection, zvariant::Value};

const APP_NAME: &str = "CrownOS";
const ICON: &str = "system-software-install";
const DESKTOP_ENTRY: &str = "crownos-settings-updates";
const DEFAULT_EXPIRY: i32 = -1;
const BODY: &str = "Open Settings to see what is new and to install it.";

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications",
    gen_blocking = false
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

/// A click launches `crownos-settings-updates.desktop`, which opens Settings
/// on System Updates. There is no `default` action: crownotify launches the
/// sender's desktop entry only when there is none.
fn notification_hints() -> HashMap<&'static str, Value<'static>> {
    HashMap::from([("desktop-entry", Value::from(DESKTOP_ENTRY))])
}

/// The session bus connection, opened on first use and again after a failure.
#[derive(Default)]
pub struct Notifier {
    connection: Option<Connection>,
}

impl Notifier {
    pub async fn announce(&mut self, version: &ReleaseVersion) -> zbus::Result<()> {
        let connection = match self.connection.take() {
            Some(connection) => connection,
            None => Connection::session().await?,
        };
        NotificationsProxy::new(&connection)
            .await?
            .notify(
                APP_NAME,
                0,
                ICON,
                &format!("CrownOS {version} is available"),
                BODY,
                &[],
                notification_hints(),
                DEFAULT_EXPIRY,
            )
            .await?;
        self.connection = Some(connection);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_only_hint_names_the_settings_entry() {
        let hints = notification_hints();
        assert_eq!(hints.len(), 1);
        assert_eq!(
            hints
                .get("desktop-entry")
                .and_then(|value| <&str>::try_from(value).ok()),
            Some("crownos-settings-updates")
        );
    }
}
