use tracing::info;
use wm_common::{DisplayState, HideMethod};
use wm_platform::NativeWindow;

use crate::{
  commands::window::manage_window,
  traits::{CommonGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn handle_window_shown(
  native_window: NativeWindow,
  state: &mut WmState,
  config: &mut UserConfig,
) -> anyhow::Result<()> {
  let found_window = state.window_from_native(&native_window);

  if let Some(window) = found_window {
    info!("Window shown: {window}");

    // Acknowledge shows (including duplicates) without redrawing the tab
    // stack and issuing another show/hide cycle. Unexpected shows of an
    // inactive tab or hidden workspace still need visibility correction.
    let should_be_visible = window
      .workspace()
      .is_some_and(|workspace| workspace.is_displayed())
      && window.is_active_tab_descendant();

    if should_be_visible
      && (window.display_state() == DisplayState::Shown
        || (config.value.general.hide_method != HideMethod::PlaceInCorner
          && window.display_state() == DisplayState::Showing))
    {
      window.set_display_state(DisplayState::Shown);
    } else {
      state.pending_sync.queue_container_to_redraw(window);
    }
  } else if !state.ignored_windows.contains(&native_window) {
    // If the window is not managed and not explicitly ignored, manage it.
    manage_window(native_window, None, state, config)?;
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{
    models::{Monitor, TabbedContainer, TilingWindow, Workspace},
    test_utils::state_with_monitors,
    traits::CommonGetters,
  };

  #[test]
  fn repeated_shown_notifications_of_active_tab_settle() {
    for hide_method in [
      HideMethod::Hide,
      HideMethod::Cloak,
      HideMethod::PlaceInCorner,
    ] {
      let window = TilingWindow::mock().call();
      let native = window.native().clone();
      let tabbed = TabbedContainer::mock()
        .tiling_containers(vec![window.clone().into()])
        .call();
      let workspace = Workspace::mock()
        .tiling_containers(vec![tabbed.clone().into()])
        .call();
      let mut state = state_with_monitors(vec![Monitor::mock()
        .workspaces(vec![workspace])
        .call()]);
      let mut config = UserConfig::mock();
      config.value.general.hide_method = hide_method;

      for _ in 0..32 {
        handle_window_shown(native.clone(), &mut state, &mut config)
          .unwrap();
        assert!(!state.pending_sync.has_changes());
        assert_eq!(window.display_state(), DisplayState::Shown);
        assert_eq!(state.windows().len(), 1);
        assert_eq!(tabbed.active_child().unwrap().id(), window.id());
      }
    }
  }

  #[cfg(target_os = "windows")]
  #[test]
  fn shown_notifications_still_redraw_hidden_tabs_and_workspaces() {
    use wm_common::SideArea;
    use wm_platform::NativeWindowWindowsExt;

    use crate::commands::container::set_focused_descendant;

    for hide_method in [
      HideMethod::Hide,
      HideMethod::Cloak,
      HideMethod::PlaceInCorner,
    ] {
      for location in [
        "inactive-tab",
        "nested-inactive-tab",
        "hidden-workspace",
        "left-sidebar",
        "right-sidebar",
      ] {
        let window = TilingWindow::mock().call();
        let native = window.native().clone();
        let other = TilingWindow::mock()
          .native(NativeWindow::from_handle(-100))
          .call();
        let child = if location == "nested-inactive-tab" {
          crate::models::SplitContainer::mock()
            .tiling_containers(vec![window.clone().into()])
            .call()
            .into()
        } else {
          window.clone().into()
        };
        let tabbed = TabbedContainer::mock()
          .tiling_containers(vec![other.clone().into(), child])
          .call();
        let main = Workspace::mock().call();
        let workspace = match location {
          "left-sidebar" | "right-sidebar" => Workspace::mock_side_area()
            .side(if location == "left-sidebar" {
              SideArea::Left
            } else {
              SideArea::Right
            })
            .tiling_containers(vec![tabbed.clone().into()])
            .call(),
          _ => Workspace::mock()
            .name("tabs".into())
            .tiling_containers(vec![tabbed.clone().into()])
            .call(),
        };
        let mut state = state_with_monitors(vec![Monitor::mock()
          .workspaces(vec![main.clone(), workspace.clone()])
          .call()]);
        if location.ends_with("inactive-tab") {
          set_focused_descendant(&other.into(), None);
        } else {
          set_focused_descendant(&window.clone().into(), None);
          set_focused_descendant(&main.clone().into(), None);
          if workspace.is_side_area() {
            workspace.hide_side_area();
          }
        }
        let focused = state.focused_container().unwrap().id();
        let mut config = UserConfig::mock();
        config.value.general.hide_method = hide_method.clone();

        for display_state in [
          DisplayState::Hidden,
          DisplayState::Hiding,
          DisplayState::Shown,
          DisplayState::Showing,
        ] {
          window.set_display_state(display_state);
          state.pending_sync.clear();
          handle_window_shown(native.clone(), &mut state, &mut config)
            .unwrap();
          assert!(
            state.windows_to_redraw().contains(&window.clone().into()),
            "{location}"
          );
          assert_eq!(state.focused_container().unwrap().id(), focused);
          assert_eq!(window.workspace().unwrap().id(), workspace.id());
          assert_eq!(state.windows().len(), 2);
        }
      }
    }
  }
}
