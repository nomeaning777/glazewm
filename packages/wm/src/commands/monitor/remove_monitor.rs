use anyhow::Context;
use tracing::info;
use wm_common::{SideArea, WmEvent};

use super::hide_side_area;
use crate::{
  commands::{
    container::{detach_container, move_container_within_tree},
    workspace::sort_workspaces,
  },
  models::{Monitor, Workspace},
  traits::CommonGetters,
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

  // Retain both active and previously hidden areas under a live monitor.
  // Their windows remain managed, but only an explicit move can reveal
  // them.
  let side_areas = monitor
    .children()
    .into_iter()
    .filter_map(|child| child.as_workspace().cloned())
    .filter(Workspace::is_side_area)
    .collect::<Vec<_>>();
  for area in side_areas {
    hide_side_area(&area, &target_monitor, state)?;
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
  use wm_platform::Rect;

  use super::*;
  use crate::{
    commands::container::set_focused_descendant,
    models::TilingWindow,
    test_utils::{
      assert_tree_links_and_focus_order, mixed_side_area,
      state_with_monitors,
    },
    traits::WindowGetters,
  };

  #[test]
  fn disappearing_side_area_stays_hidden_and_managed_on_monitor_removal() {
    for side in [SideArea::Left, SideArea::Right] {
      for split_first in [false, true] {
        for target_has_area in [false, true] {
          let (source_area, split, windows) =
            mixed_side_area(side, split_first);
          let regular_window = TilingWindow::mock().call();
          let source_workspace = Workspace::mock()
            .tiling_containers(vec![regular_window.clone().into()])
            .call();
          let source_monitor = Monitor::mock()
            .device_name("SOURCE".to_string())
            .workspaces(vec![
              source_area.clone(),
              source_workspace.clone(),
            ])
            .call();
          let target_workspace = Workspace::mock().call();
          let target_area = Workspace::mock_side_area().side(side).call();
          let mut target_workspaces = vec![target_workspace.clone()];
          if target_has_area {
            target_workspaces.push(target_area.clone());
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
          set_focused_descendant(&windows[0].clone().into(), None);
          let child_order = source_area.children();
          let focus_order = source_area.borrow_child_focus_order().clone();

          remove_monitor(
            source_monitor.clone(),
            &mut state,
            &UserConfig::mock(),
          )
          .unwrap();

          assert!(!source_area.is_displayed());
          assert!(!source_area.is_detached());
          assert!(source_monitor.is_detached());
          assert!(state.container_by_id(source_monitor.id()).is_none());
          assert_eq!(source_area.children(), child_order);
          assert_eq!(*source_area.borrow_child_focus_order(), focus_order);
          assert!(!split.is_detached());
          assert_eq!(target_monitor.workspaces().len(), 2);
          assert_eq!(
            target_monitor.side_area(side).map(|area| area.id()),
            target_has_area.then_some(target_area.id())
          );
          assert!(!target_area.has_children());
          assert_eq!(
            regular_window.workspace().unwrap().id(),
            source_workspace.id()
          );
          assert_eq!(
            source_workspace.monitor().unwrap().id(),
            target_monitor.id()
          );
          for window in windows {
            assert_eq!(window.workspace().unwrap().id(), source_area.id());
            assert_eq!(
              window.monitor().unwrap().id(),
              target_monitor.id()
            );
            assert!(state.container_by_id(window.id()).is_some());
            assert!(window.has_pending_dpi_adjustment());
          }
          assert!(state
            .focused_container()
            .unwrap()
            .workspace()
            .unwrap()
            .is_displayed());
          assert_tree_links_and_focus_order(
            &state.root_container.clone().into(),
          );
        }
      }
    }
  }
}
