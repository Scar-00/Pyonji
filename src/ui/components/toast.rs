//! Notifications, on the same glass as every other floating surface.

use gpui::{Anchor, App, Styled as _};
use gpui_component::{
    Icon,
    notification::{Notification, NotificationType},
};

use super::PyStyled as _;
use crate::assets::PyonjiAsset;
use crate::ui::theme::PyTheme as _;

/// A notification in the top right corner, marked with `icon`.
pub fn toast(icon: Icon, cx: &App) -> Notification {
    Notification::new()
        .icon(icon)
        .floating(cx)
        .placement(Anchor::TopRight)
}

/// The icon for a notification of `kind`, in its status colour.
pub fn status_icon(kind: NotificationType, cx: &App) -> Icon {
    let theme = cx.theme();
    let (asset, color) = match kind {
        NotificationType::Error => (PyonjiAsset::NotificationError, theme.error),
        NotificationType::Warning => (PyonjiAsset::NotificationWarning, theme.warning),
        NotificationType::Success => (PyonjiAsset::NotificationSuccess, theme.success),
        NotificationType::Info => (PyonjiAsset::NotificationInfo, theme.info),
    };
    Icon::new(asset).text_color(color)
}
