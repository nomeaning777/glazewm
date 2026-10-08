use wm_common::SideArea;

use crate::{
  commands::container::{attach_container, detach_container},
  models::{Monitor, Workspace},
  traits::{CommonGetters, WindowGetters},
  wm_state::WmState,
};

/// Retains a disappeared sidebar in the live tree for queries and explicit
/// Move commands. Keeping the workspace intact preserves split/tab layout.
pub(super) fn hide_side_area(
  area: &Workspace,
  target_monitor: &Monitor,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let focused_before = state.focused_container();
  let changed_monitor = area
    .monitor()
    .is_some_and(|monitor| monitor.id() != target_monitor.id());
  area.hide_side_area();
  detach_container(area.clone().into())?;

  if area.has_children() {
    attach_container(
      &area.clone().into(),
      &target_monitor.clone().into(),
      Some(
        target_monitor.child_count()
          - usize::from(
            target_monitor.side_area(SideArea::Right).is_some(),
          ),
      ),
    )?;
    if changed_monitor {
      for window in area
        .descendants()
        .filter_map(|child| child.as_window_container().ok())
      {
        window.set_has_pending_dpi_adjustment(true);
      }
    }
    state.pending_sync.queue_container_to_redraw(area.clone());
  }

  if focused_before != state.focused_container() {
    state.pending_sync.queue_focus_change();
  }
  state
    .pending_sync
    .queue_container_to_redraw(target_monitor.clone());
  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::{
    FloatingStateConfig, FullscreenStateConfig, WindowState,
  };
  use wm_platform::{Display, LengthValue};

  use super::*;
  use crate::{
    commands::{
      container::set_focused_descendant,
      monitor::{add_monitor, ensure_side_areas, remove_monitor},
      window::move_window_to_workspace,
    },
    models::{
      NativeMonitorProperties, NonTilingWindow, TabbedContainer,
      TilingWindow, WorkspaceTarget,
    },
    test_utils::{
      assert_tree_links_and_focus_order, mixed_side_area,
      state_with_monitors,
    },
    user_config::UserConfig,
  };

  #[test]
  fn hidden_layout_and_states_survive_until_explicit_move() {
    for side in [SideArea::Left, SideArea::Right] {
      let (area, split, tiling_windows) = mixed_side_area(side, false);
      let tab = TilingWindow::mock().call();
      let tabbed = TabbedContainer::mock()
        .tiling_containers(vec![tab.clone().into()])
        .call();
      attach_container(
        &tabbed.clone().into(),
        &split.clone().into(),
        None,
      )
      .unwrap();
      let states = [
        WindowState::Floating(FloatingStateConfig::default()),
        WindowState::Fullscreen(FullscreenStateConfig {
          maximized: true,
          ..FullscreenStateConfig::default()
        }),
        WindowState::Minimized,
      ];
      let windows = states
        .iter()
        .map(|state| {
          let window = NonTilingWindow::mock().state(state.clone()).call();
          attach_container(
            &window.clone().into(),
            &area.clone().into(),
            None,
          )
          .unwrap();
          window
        })
        .collect::<Vec<_>>();
      let workspace = Workspace::mock().name("1".to_string()).call();
      let monitor = Monitor::mock()
        .workspaces(vec![area.clone(), workspace.clone()])
        .call();
      let mut state = state_with_monitors(vec![monitor.clone()]);
      let mut config = UserConfig::mock();
      set_focused_descendant(&tab.clone().into(), None);
      let children = area.children();
      let split_children = split.children();

      ensure_side_areas(&monitor, &mut state, &config).unwrap();
      config.value.side_areas.left = LengthValue::from_px(300);
      config.value.side_areas.right = LengthValue::from_px(300);
      ensure_side_areas(&monitor, &mut state, &config).unwrap();

      assert!(!area.is_displayed());
      assert_eq!(area.children(), children);
      assert_eq!(split.children(), split_children);
      assert_eq!(tabbed.children()[0].id(), tab.id());
      assert!(area.to_dto().is_ok());
      assert!(state.pending_sync.needs_focus_update());
      assert_eq!(state.focused_container().unwrap().id(), workspace.id());
      for (window, expected) in windows.iter().zip(&states) {
        assert_eq!(window.state(), *expected);
        assert_eq!(window.workspace().unwrap().id(), area.id());
      }
      for window in state.windows() {
        assert!(state.windows_to_redraw().contains(&window));
        let id = window.id();
        let original_state = window.state();
        move_window_to_workspace(
          window,
          WorkspaceTarget::Name("1".to_string()),
          &mut state,
          &config,
        )
        .unwrap();
        let recovered = state
          .container_by_id(id)
          .unwrap()
          .as_window_container()
          .unwrap();
        assert_eq!(recovered.workspace().unwrap().id(), workspace.id());
        assert_eq!(recovered.state(), original_state);
      }
      assert!(!area.has_children());
      assert_eq!(
        state.windows().len(),
        tiling_windows.len() + windows.len() + 1
      );
      assert_tree_links_and_focus_order(
        &state.root_container.clone().into(),
      );
    }
  }

  #[test]
  fn hidden_side_area_survives_repeated_disconnects_and_reconnects() {
    for side in [SideArea::Left, SideArea::Right] {
      let (area, _, windows) = mixed_side_area(side, true);
      let source = Monitor::mock()
        .device_name("SOURCE".to_string())
        .workspaces(vec![area.clone(), Workspace::mock().call()])
        .call();
      let mut host = Monitor::mock()
        .device_name("HOST".to_string())
        .workspaces(vec![Workspace::mock().call()])
        .call();
      let mut state =
        state_with_monitors(vec![source.clone(), host.clone()]);
      let mut config = UserConfig::mock();
      config.value.side_areas.left = LengthValue::from_px(300);
      config.value.side_areas.right = LengthValue::from_px(300);
      remove_monitor(source, &mut state, &config).unwrap();

      for _ in 0..3 {
        let reconnected = add_monitor(
          Display::mock(),
          NativeMonitorProperties::mock()
            .device_name("SOURCE".to_string())
            .call(),
          &mut state,
          &config,
        )
        .unwrap();
        let regular = Workspace::mock().call();
        attach_container(
          &regular.clone().into(),
          &reconnected.clone().into(),
          None,
        )
        .unwrap();
        assert!(!reconnected.side_area(side).unwrap().has_children());
        assert!(!area.is_displayed());
        let removed_id = host.id();
        remove_monitor(host, &mut state, &config).unwrap();
        assert!(state.container_by_id(removed_id).is_none());
        for window in &windows {
          assert_eq!(window.workspace().unwrap().id(), area.id());
          assert_eq!(window.monitor().unwrap().id(), reconnected.id());
          assert!(state.container_by_id(window.id()).is_some());
        }
        assert!(!area.is_displayed());
        assert_eq!(state.windows().len(), windows.len());
        assert_tree_links_and_focus_order(
          &state.root_container.clone().into(),
        );
        host = reconnected;
      }
    }
  }
}
