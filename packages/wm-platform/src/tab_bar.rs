use crate::{Dispatcher, Rect};

/// A single tab displayed in a [`TabBar`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabBarItem {
  pub id: String,
  pub title: String,
  pub is_active: bool,
}

/// A native, clickable tab bar.
///
/// All platform UI work is dispatched to the event loop thread.
pub struct TabBar {
  inner: platform::TabBar,
}

impl TabBar {
  /// Creates a tab bar and registers its click handler.
  pub fn new<F>(
    dispatcher: &Dispatcher,
    on_click: F,
  ) -> crate::Result<Self>
  where
    F: Fn(String) + Send + Sync + 'static,
  {
    Ok(Self {
      inner: platform::TabBar::new(dispatcher, on_click)?,
    })
  }

  /// Shows or updates the tab bar.
  pub fn update(
    &mut self,
    rect: &Rect,
    items: &[TabBarItem],
  ) -> crate::Result<()> {
    self.inner.update(rect, items)
  }

  /// Hides the tab bar.
  pub fn hide(&mut self) -> crate::Result<()> {
    self.inner.hide()
  }

  /// Hides and destroys the tab bar's native window.
  pub fn close(&mut self) -> crate::Result<()> {
    self.inner.close()
  }
}

#[cfg(target_os = "windows")]
#[path = "platform_impl/windows/tab_bar.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "platform_impl/macos/tab_bar.rs"]
mod platform;
