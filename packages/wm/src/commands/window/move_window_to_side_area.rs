use anyhow::Context;
use wm_common::{SideArea, WindowState};

use crate::{
  commands::container::{
    move_container_within_tree, set_focused_descendant,
  },
  models::WindowContainer,
  traits::{CommonGetters, PositionGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

/// Moves a window to a persistent side area on its current monitor.
pub fn move_window_to_side_area(
  window: WindowContainer,
  side: SideArea,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let current_workspace =
    window.workspace().context("Window has no workspace.")?;
  let monitor = window.monitor().context("Window has no monitor.")?;
  let target_area = monitor.side_area(side).with_context(|| {
    let native_properties = monitor.native_properties();
    let device_name = native_properties.device_name;
    let side_areas = &config.value.side_areas;

    if side_areas.matches_monitor(
      &device_name,
      native_properties.hardware_id.as_deref(),
    ) {
      let configured_width = match side {
        SideArea::Left => &side_areas.left,
        SideArea::Right => &side_areas.right,
      };

      if configured_width.amount <= 0.0 {
        format!(
          "The {side:?} side area is disabled on monitor {device_name:?}. \
           Configure a positive width first."
        )
      } else {
        format!(
          "The {side:?} side area is configured but unavailable on monitor \
          {device_name:?}. Reload the config and try again."
        )
      }
    } else {
      format!(
        "The {side:?} side area is unavailable on monitor {device_name:?} \
         because it does not match side_areas.match."
      )
    }
  })?;

  if current_workspace.id() == target_area.id() {
    return Ok(());
  }

  let was_focused = window.has_focus(None);
  let insertion_sibling = target_area
    .descendant_focus_order()
    .find(crate::models::Container::is_tiling_window);

  let target_parent = insertion_sibling
    .as_ref()
    .and_then(CommonGetters::parent)
    .unwrap_or_else(|| target_area.clone().into());
  let target_index = insertion_sibling.map_or_else(
    || target_area.child_count(),
    |sibling| sibling.index() + 1,
  );

  if !window.is_tiling_window() {
    let area_rect = target_area.to_rect()?;
    let placement = window
      .floating_placement()
      .clamp_size(area_rect.width(), area_rect.height())
      .translate_to_center(&area_rect);
    window.set_floating_placement(placement);

    // A non-tiling window can retain the position of its previous tiling
    // layout. Clear it when crossing into a side area so a later
    // `set-tiling` keeps the window in this independent region instead of
    // restoring it into the regular workspace.
    if let WindowContainer::NonTilingWindow(window) = &window {
      window.set_insertion_target(None);
    }
  }

  move_container_within_tree(
    &window.clone().into(),
    &target_parent,
    target_index,
    state,
  )?;

  if let WindowState::Fullscreen(mut fullscreen) = window.state() {
    fullscreen.maximized = false;
    window.set_state(WindowState::Fullscreen(fullscreen));
  }

  if was_focused {
    set_focused_descendant(&window.clone().into(), None);
    state.pending_sync.queue_focus_change();
  }

  if window.state() == WindowState::Tiling {
    state
      .pending_sync
      .queue_containers_to_redraw(current_workspace.tiling_children())
      .queue_containers_to_redraw(target_area.tiling_children());
  } else {
    state.pending_sync.queue_container_to_redraw(window);
  }

  if let Some(displayed_workspace) = monitor.displayed_workspace() {
    state
      .pending_sync
      .queue_container_to_redraw(displayed_workspace);
  }

  state.pending_sync.queue_workspace_to_reorder(target_area);
  Ok(())
}

#[cfg(test)]
mod tests {
  use tokio::sync::mpsc;
  use wm_common::FloatingStateConfig;
  use wm_platform::{Dispatcher, LengthValue};

  use super::*;
  use crate::{
    commands::{
      container::{attach_container, set_focused_descendant},
      window::update_window_state,
    },
    models::{Monitor, TilingWindow, Workspace},
    user_config::UserConfig,
  };

  #[test]
  fn moves_window_without_switching_the_regular_workspace() {
    let window = TilingWindow::mock().call();
    let regular_workspace = Workspace::mock()
      .tiling_containers(vec![window.clone().into()])
      .call();
    let side_area = Workspace::mock_side_area()
      .side(SideArea::Left)
      .width(LengthValue::from_px(300))
      .call();
    let monitor = Monitor::mock()
      .workspaces(vec![regular_workspace.clone(), side_area.clone()])
      .call();
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (exit_tx, _exit_rx) = mpsc::unbounded_channel();
    let mut state = WmState::new(Dispatcher::mock(), event_tx, exit_tx);
    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();
    set_focused_descendant(&window.clone().into(), None);

    move_window_to_side_area(
      window.clone().into(),
      SideArea::Left,
      &mut state,
      &UserConfig::mock(),
    )
    .unwrap();

    assert_eq!(
      window.workspace().map(|workspace| workspace.id()),
      Some(side_area.id())
    );
    assert_eq!(
      monitor
        .displayed_workspace()
        .map(|workspace| workspace.id()),
      Some(regular_workspace.id())
    );
    assert_eq!(
      state.focused_container().map(|container| container.id()),
      Some(window.id())
    );
    assert_eq!(window.to_rect().unwrap().width(), 300);
  }

  #[test]
  fn returning_to_tiling_keeps_window_in_side_area() {
    let window = TilingWindow::mock().call();
    let regular_workspace = Workspace::mock()
      .tiling_containers(vec![window.clone().into()])
      .call();
    let side_area = Workspace::mock_side_area()
      .side(SideArea::Left)
      .width(LengthValue::from_px(300))
      .call();
    let monitor = Monitor::mock()
      .workspaces(vec![regular_workspace, side_area.clone()])
      .call();
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (exit_tx, _exit_rx) = mpsc::unbounded_channel();
    let mut state = WmState::new(Dispatcher::mock(), event_tx, exit_tx);
    let config = UserConfig::mock();
    attach_container(
      &monitor.into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    let floating = update_window_state(
      window.into(),
      WindowState::Floating(FloatingStateConfig::default()),
      &mut state,
      &config,
    )
    .unwrap();
    move_window_to_side_area(
      floating.clone(),
      SideArea::Left,
      &mut state,
      &UserConfig::mock(),
    )
    .unwrap();

    let restored = update_window_state(
      floating,
      WindowState::Tiling,
      &mut state,
      &config,
    )
    .unwrap();

    assert_eq!(
      restored.workspace().map(|workspace| workspace.id()),
      Some(side_area.id())
    );
  }

  #[test]
  fn missing_side_area_error_explains_selector_or_width() {
    let window = TilingWindow::mock().call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![window.clone().into()])
      .call();
    let monitor = Monitor::mock()
      .device_name("DISPLAY2".to_string())
      .workspaces(vec![workspace])
      .call();
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (exit_tx, _exit_rx) = mpsc::unbounded_channel();
    let mut state = WmState::new(Dispatcher::mock(), event_tx, exit_tx);
    attach_container(
      &monitor.into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    let mut config = UserConfig::mock();
    config.value.side_areas.match_monitor =
      Some(vec![wm_common::MonitorMatchConfig {
        device_name: Some(wm_common::MatchType::Equals {
          equals: "DISPLAY1".to_string(),
        }),
        hardware_id: None,
      }]);
    let error = move_window_to_side_area(
      window.clone().into(),
      SideArea::Left,
      &mut state,
      &config,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("DISPLAY2"), "{error}");
    assert!(error.contains("side_areas.match"), "{error}");

    config.value.side_areas.match_monitor = None;
    let error = move_window_to_side_area(
      window.into(),
      SideArea::Left,
      &mut state,
      &config,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("positive width"), "{error}");
    assert!(!error.contains("does not match"), "{error}");
  }
}
