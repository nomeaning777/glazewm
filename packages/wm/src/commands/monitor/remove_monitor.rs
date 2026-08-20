use anyhow::Context;
use tracing::info;
use wm_common::{SideArea, WmEvent};

use crate::{
  commands::{
    container::{detach_container, move_container_within_tree},
    workspace::sort_workspaces,
  },
  models::Monitor,
  traits::{CommonGetters, PositionGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

#[allow(clippy::needless_pass_by_value)]
pub fn remove_monitor(
  monitor: Monitor,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  info!("Removing monitor: {monitor}");

  let target_monitor = state
    .monitors()
    .into_iter()
    .find(|m| m.id() != monitor.id())
    .context("No target monitor to move workspaces.")?;

  // Side areas are monitor-local, so move their contents into the
  // corresponding area on the remaining monitor.
  for side in [SideArea::Left, SideArea::Right] {
    let Some(source_area) = monitor.side_area(side) else {
      continue;
    };
    let target_parent = target_monitor
      .side_area(side)
      .or_else(|| target_monitor.displayed_workspace())
      .context("No target region for side-area windows.")?;

    for child in source_area.children() {
      let moved_windows = child
        .self_and_descendants()
        .filter_map(|container| container.as_window_container().ok())
        .collect::<Vec<_>>();

      move_container_within_tree(
        &child,
        &target_parent.clone().into(),
        target_parent.child_count(),
        state,
      )?;

      for window in moved_windows {
        window.set_has_pending_dpi_adjustment(true);
        window.set_floating_placement(
          window
            .floating_placement()
            .translate_to_center(&target_parent.to_rect()?),
        );
      }
    }
  }

  // Avoid moving empty workspaces.
  let workspaces_to_move =
    monitor.workspaces().into_iter().filter(|workspace| {
      workspace.has_children() || workspace.config().keep_alive
    });

  for workspace in workspaces_to_move {
    // Move workspace to target monitor.
    move_container_within_tree(
      &workspace.clone().into(),
      &target_monitor.clone().into(),
      target_monitor.child_count()
        - usize::from(target_monitor.side_area(SideArea::Right).is_some()),
      state,
    )?;

    sort_workspaces(&target_monitor, config)?;

    state.emit_event(WmEvent::WorkspaceUpdated {
      updated_workspace: workspace.to_dto()?,
    });
  }

  detach_container(monitor.clone().into())?;

  state.emit_event(WmEvent::MonitorRemoved {
    removed_id: monitor.id(),
    removed_device_name: monitor.native_properties().device_name,
  });

  Ok(())
}
