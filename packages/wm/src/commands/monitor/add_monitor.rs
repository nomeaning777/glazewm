use anyhow::Context;
use tracing::info;
use wm_common::{SideArea, WindowState, WmEvent};
use wm_platform::Display;

use crate::{
  commands::{
    container::{
      attach_container, detach_container, flatten_child_split_containers,
      move_container_within_tree,
    },
    workspace::{activate_workspace, sort_workspaces},
  },
  models::{Monitor, NativeMonitorProperties, Workspace},
  traits::{CommonGetters, PositionGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn add_monitor(
  native_display: Display,
  native_properties: NativeMonitorProperties,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<Monitor> {
  // Create `Monitor` instance. This uses the working area of the monitor
  // instead of the bounds of the display. The working area excludes
  // taskbars and other reserved display space.
  let monitor = Monitor::new(native_display, native_properties);

  attach_container(
    &monitor.clone().into(),
    &state.root_container.clone().into(),
    None,
  )?;

  ensure_side_areas(&monitor, state, config)?;

  info!("Monitor added: {monitor}");

  state.emit_event(WmEvent::MonitorAdded {
    added_monitor: monitor.to_dto()?,
  });

  Ok(monitor)
}

/// Creates, updates, or removes persistent side areas for a monitor.
pub fn ensure_side_areas(
  monitor: &Monitor,
  state: &WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  for (side, width) in [
    (SideArea::Left, config.value.side_areas.left.clone()),
    (SideArea::Right, config.value.side_areas.right.clone()),
  ] {
    let existing = monitor.side_area(side);

    if width.amount <= 0.0 {
      if let Some(area) = existing {
        if let Some(target_workspace) = monitor.displayed_workspace() {
          let children = area.children().into_iter().collect::<Vec<_>>();
          for child in &children {
            move_container_within_tree(
              child,
              &target_workspace.clone().into(),
              target_workspace.child_count(),
              state,
            )?;
          }
          flatten_child_split_containers(
            &target_workspace.clone().into(),
          )?;
        }
        detach_container(area.into())?;
      }
      continue;
    }

    if let Some(area) = existing {
      area.set_side_area_config(
        width,
        config.value.side_areas.scale_with_dpi,
      );
      area.set_gaps_config(config.value.gaps.clone());
    } else {
      let area = Workspace::new_side_area(
        side,
        width,
        config.value.side_areas.scale_with_dpi,
        config.value.gaps.clone(),
      );
      attach_container(
        &area.into(),
        &monitor.clone().into(),
        Some(match side {
          SideArea::Left => 0,
          SideArea::Right => monitor.child_count(),
        }),
      )?;
    }
  }

  if monitor.has_side_areas() {
    for window in monitor
      .descendants()
      .filter_map(|container| container.as_window_container().ok())
    {
      if let WindowState::Fullscreen(mut fullscreen) = window.state() {
        fullscreen.maximized = false;
        window.set_state(WindowState::Fullscreen(fullscreen));
      }
    }
  }

  Ok(())
}

pub fn move_bounded_workspaces_to_new_monitor(
  monitor: &Monitor,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let bound_workspace_configs = config
    .value
    .workspaces
    .iter()
    .filter(|config| {
      config.bind_to_monitor.is_some_and(|monitor_index| {
        monitor.index() == monitor_index as usize
      })
    })
    .collect::<Vec<_>>();

  for workspace_config in bound_workspace_configs {
    let existing_workspace =
      state.workspace_by_name(&workspace_config.name);

    if let Some(existing_workspace) = existing_workspace {
      // Move workspaces that should be bound to the newly added monitor.
      move_workspace_to_monitor(
        &existing_workspace,
        monitor,
        state,
        config,
      )?;
    } else if workspace_config.keep_alive {
      // Activate all `keep_alive` workspaces for this monitor.
      activate_workspace(
        Some(&workspace_config.name),
        Some(monitor.clone()),
        state,
        config,
      )?;
    }
  }

  // Make sure the monitor has at least one workspace. This will
  // automatically prioritize bound workspace configs and fall back to the
  // first available one if needed.
  if monitor.workspaces().is_empty() {
    activate_workspace(None, Some(monitor.clone()), state, config)?;
  }

  Ok(())
}

// TODO: Move to its own file once `swap-workspace` PR is merged.
// Ref: https://github.com/glzr-io/glazewm/pull/980.
pub fn move_workspace_to_monitor(
  workspace: &Workspace,
  target_monitor: &Monitor,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let origin_monitor = workspace.monitor().context("No monitor.")?;

  move_container_within_tree(
    &workspace.clone().into(),
    &target_monitor.clone().into(),
    target_monitor.child_count()
      - usize::from(target_monitor.side_area(SideArea::Right).is_some()),
    state,
  )?;

  let windows = workspace
    .descendants()
    .filter_map(|descendant| descendant.as_window_container().ok());

  for window in windows {
    window.set_has_pending_dpi_adjustment(true);

    window.set_floating_placement(
      window
        .floating_placement()
        .translate_to_center(&workspace.to_rect()?),
    );
  }

  // Get currently displayed workspace on the target monitor.
  let displayed_workspace = target_monitor
    .displayed_workspace()
    .context("No displayed workspace.")?;

  state
    .pending_sync
    .queue_container_to_redraw(workspace.clone())
    .queue_container_to_redraw(displayed_workspace);

  match origin_monitor.workspaces().len() {
    0 => {
      // Prevent origin monitor from having no workspaces.
      activate_workspace(None, Some(origin_monitor), state, config)?;
    }
    _ => {
      // Redraw the workspace on the origin monitor.
      state.pending_sync.queue_container_to_redraw(
        origin_monitor
          .displayed_workspace()
          .context("No displayed workspace.")?,
      );
    }
  }

  sort_workspaces(target_monitor, config)?;

  state.emit_event(WmEvent::WorkspaceUpdated {
    updated_workspace: workspace.to_dto()?,
  });

  Ok(())
}

#[cfg(test)]
mod tests {
  use tokio::sync::mpsc;
  use wm_common::{FloatingStateConfig, GapsConfig};
  use wm_platform::{Dispatcher, LengthValue};

  use super::*;
  use crate::{
    commands::container::attach_container,
    models::{SplitContainer, TilingWindow},
  };

  fn state_with_monitor(monitor: Monitor) -> WmState {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (exit_tx, _exit_rx) = mpsc::unbounded_channel();
    let state = WmState::new(Dispatcher::mock(), event_tx, exit_tx);
    attach_container(
      &monitor.into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();
    state
  }

  #[test]
  fn disabling_side_area_moves_contents_back_and_flattens_layout() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let split = SplitContainer::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();
    let side_area = Workspace::mock_side_area()
      .side(SideArea::Left)
      .tiling_containers(vec![split.into()])
      .call();
    let regular_workspace = Workspace::mock().call();
    let monitor = Monitor::mock()
      .workspaces(vec![side_area.clone(), regular_workspace.clone()])
      .call();
    let state = state_with_monitor(monitor.clone());
    let mut config = UserConfig::mock();
    config.value.side_areas.left = LengthValue::from_px(0);

    ensure_side_areas(&monitor, &state, &config).unwrap();

    assert!(monitor.side_area(SideArea::Left).is_none());
    assert_eq!(
      regular_workspace
        .tiling_children()
        .map(|child| child.id())
        .collect::<Vec<_>>(),
      vec![first.id(), second.id()]
    );
  }

  #[test]
  fn disabling_side_area_preserves_non_tiling_windows() {
    let window = crate::models::NonTilingWindow::mock()
      .state(WindowState::Floating(FloatingStateConfig::default()))
      .call();
    let side_area = Workspace::mock_side_area()
      .side(SideArea::Right)
      .non_tiling_windows(vec![window.clone()])
      .call();
    let regular_workspace =
      Workspace::mock().gaps_config(GapsConfig::default()).call();
    let monitor = Monitor::mock()
      .workspaces(vec![regular_workspace.clone(), side_area])
      .call();
    let state = state_with_monitor(monitor.clone());
    let mut config = UserConfig::mock();
    config.value.side_areas.right = LengthValue::from_px(0);

    ensure_side_areas(&monitor, &state, &config).unwrap();

    assert_eq!(
      window.workspace().map(|workspace| workspace.id()),
      Some(regular_workspace.id())
    );
  }
}
