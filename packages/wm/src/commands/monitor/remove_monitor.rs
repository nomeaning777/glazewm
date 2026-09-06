use anyhow::Context;
use tracing::info;
use wm_common::{SideArea, WmEvent};

use super::move_side_area_contents;
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

    let moved_windows =
      move_side_area_contents(&source_area, &target_parent, state)?;
    for window in moved_windows {
      window.set_has_pending_dpi_adjustment(true);
      window.set_floating_placement(
        window
          .floating_placement()
          .translate_to_center(&target_parent.to_rect()?),
      );
    }
    detach_container(source_area.into())?;
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

#[cfg(test)]
mod tests {
  use wm_common::GapsConfig;
  use wm_platform::Rect;

  use super::*;
  use crate::{
    models::Workspace,
    test_utils::{
      assert_tree_links_and_focus_order, mixed_side_area,
      state_with_monitors,
    },
  };

  #[test]
  fn removal_evacuates_mixed_side_area_children_in_both_orders() {
    for split_first in [false, true] {
      let (source_area, split, windows) =
        mixed_side_area(SideArea::Left, split_first);
      let source_workspace = Workspace::mock().call();
      let source_monitor = Monitor::mock()
        .device_name("SOURCE".to_string())
        .dpi(96)
        .workspaces(vec![source_area.clone(), source_workspace])
        .call();

      let target_workspace =
        Workspace::mock().gaps_config(GapsConfig::default()).call();
      let target_area = (!split_first)
        .then(|| Workspace::mock_side_area().side(SideArea::Left).call());
      let mut target_workspaces = vec![target_workspace.clone()];
      if let Some(target_area) = &target_area {
        target_workspaces.insert(0, target_area.clone());
      }
      let target_monitor = Monitor::mock()
        .device_name("TARGET".to_string())
        .bounds(Rect::from_xy(1680, 0, 1920, 1080))
        .working_area(Rect::from_xy(1680, 0, 1920, 1040))
        .dpi(144)
        .workspaces(target_workspaces)
        .call();
      let mut state = state_with_monitors(vec![
        source_monitor.clone(),
        target_monitor.clone(),
      ]);
      let expected_workspace = target_area.unwrap_or(target_workspace);
      let target_rect = expected_workspace.to_rect().unwrap();
      let expected_placements = windows
        .iter()
        .map(|window| {
          window
            .floating_placement()
            .translate_to_center(&target_rect)
        })
        .collect::<Vec<_>>();

      remove_monitor(
        source_monitor.clone(),
        &mut state,
        &UserConfig::mock(),
      )
      .unwrap();

      assert!(source_monitor.is_detached());
      assert!(state
        .monitors()
        .iter()
        .all(|monitor| { monitor.id() != source_monitor.id() }));
      assert!(state.root_container.descendants().all(|container| {
        container.id() != source_area.id()
          && container.id() != source_monitor.id()
      }));
      assert!(source_area.is_detached());
      assert!(!source_area.has_children());
      assert!(source_area.borrow_child_focus_order().is_empty());
      if split.is_detached() {
        assert!(!split.has_children());
      } else {
        assert_eq!(
          split.workspace().map(|workspace| workspace.id()),
          Some(expected_workspace.id())
        );
      }
      for (window, expected_placement) in
        windows.into_iter().zip(expected_placements)
      {
        assert_eq!(
          window.workspace().map(|workspace| workspace.id()),
          Some(expected_workspace.id())
        );
        assert!(window.has_pending_dpi_adjustment());
        assert_eq!(window.floating_placement(), expected_placement);
      }
      assert_tree_links_and_focus_order(
        &state.root_container.clone().into(),
      );
      assert_tree_links_and_focus_order(&source_monitor.clone().into());
    }
  }
}
