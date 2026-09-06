use tracing::info;
use wm_common::WmEvent;
use wm_platform::Display;

use crate::{
  commands::monitor::ensure_side_areas,
  models::{Monitor, NativeMonitorProperties},
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn update_monitor(
  monitor: &Monitor,
  native_display: &Display,
  native_properties: NativeMonitorProperties,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  monitor.set_native(native_display.clone());
  monitor.set_native_properties(native_properties);
  ensure_side_areas(monitor, state, config)?;

  info!("Monitor updated: {monitor}");

  // TODO: Check that a property on the monitor actually changed.
  state.emit_event(WmEvent::MonitorUpdated {
    updated_monitor: monitor.to_dto()?,
  });

  Ok(())
}

#[cfg(test)]
mod tests {
  use tokio::sync::mpsc;
  use wm_common::SideArea;
  use wm_platform::{Dispatcher, LengthValue};

  use super::*;
  use crate::{
    commands::container::{attach_container, set_focused_descendant},
    models::{TilingWindow, Workspace},
    test_utils::assert_tree_links_and_focus_order,
    traits::CommonGetters,
    user_config::UserConfig,
  };

  fn update_mock_monitor(
    monitor: &Monitor,
    device_name: &str,
    hardware_id: &str,
    state: &mut WmState,
    config: &UserConfig,
  ) {
    update_monitor(
      monitor,
      &Display::mock(),
      NativeMonitorProperties::mock()
        .device_name(device_name.to_string())
        .hardware_id(hardware_id.to_string())
        .call(),
      state,
      config,
    )
    .unwrap();
  }

  #[test]
  fn device_name_change_reconciles_side_areas() {
    let workspace = Workspace::mock().call();
    let monitor = Monitor::mock()
      .device_name("DISPLAY2".to_string())
      .workspaces(vec![workspace])
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
    let mut config = UserConfig::mock();
    config.value.side_areas.left = LengthValue::from_px(300);
    config.value.side_areas.match_monitor =
      Some(vec![wm_common::MonitorMatchConfig {
        device_name: Some(wm_common::MatchType::Equals {
          equals: "DISPLAY1".to_string(),
        }),
        hardware_id: None,
      }]);

    update_monitor(
      &monitor,
      &Display::mock(),
      NativeMonitorProperties::mock()
        .device_name("DISPLAY1".to_string())
        .call(),
      &mut state,
      &config,
    )
    .unwrap();
    assert!(monitor.side_area(SideArea::Left).is_some());

    update_monitor(
      &monitor,
      &Display::mock(),
      NativeMonitorProperties::mock()
        .device_name("DISPLAY2".to_string())
        .call(),
      &mut state,
      &config,
    )
    .unwrap();
    assert!(monitor.side_area(SideArea::Left).is_none());
  }

  #[test]
  #[allow(clippy::too_many_lines)]
  fn hardware_id_selection_survives_device_name_changes_and_reconciles() {
    let left_window = TilingWindow::mock().call();
    let right_window = TilingWindow::mock().call();
    let left_area = Workspace::mock_side_area()
      .side(SideArea::Left)
      .tiling_containers(vec![left_window.clone().into()])
      .call();
    let right_area = Workspace::mock_side_area()
      .side(SideArea::Right)
      .tiling_containers(vec![right_window.clone().into()])
      .call();
    let selected_workspace = Workspace::mock().call();
    let selected_monitor = Monitor::mock()
      .device_name(r"\\.\DISPLAY1".to_string())
      .hardware_id("DEL439E".to_string())
      .workspaces(vec![
        left_area.clone(),
        selected_workspace.clone(),
        right_area.clone(),
      ])
      .call();
    let other_workspace = Workspace::mock().call();
    let other_monitor = Monitor::mock()
      .device_name(r"\\.\DISPLAY2".to_string())
      .hardware_id("ACR1234".to_string())
      .workspaces(vec![other_workspace])
      .call();
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (exit_tx, _exit_rx) = mpsc::unbounded_channel();
    let mut state = WmState::new(Dispatcher::mock(), event_tx, exit_tx);
    for monitor in [&selected_monitor, &other_monitor] {
      attach_container(
        &monitor.clone().into(),
        &state.root_container.clone().into(),
        None,
      )
      .unwrap();
    }
    set_focused_descendant(&left_window.clone().into(), None);
    let initial_area_ids = [left_area.id(), right_area.id()];
    let mut config = UserConfig::mock();
    config.value = serde_yaml::from_str(
      r"
side_areas:
  left: 300px
  right: 300px
  match:
    - hardware_id: { equals: DEL439E }
",
    )
    .unwrap();

    update_mock_monitor(
      &selected_monitor,
      r"\\.\DISPLAY2",
      "DEL439E",
      &mut state,
      &config,
    );
    update_mock_monitor(
      &other_monitor,
      r"\\.\DISPLAY1",
      "ACR1234",
      &mut state,
      &config,
    );

    assert_eq!(
      selected_monitor.side_area(SideArea::Left).unwrap().id(),
      initial_area_ids[0]
    );
    assert_eq!(
      selected_monitor.side_area(SideArea::Right).unwrap().id(),
      initial_area_ids[1]
    );
    assert!(!other_monitor.has_side_areas());

    update_mock_monitor(
      &selected_monitor,
      r"\\.\DISPLAY2",
      "SAM0001",
      &mut state,
      &config,
    );

    assert!(!selected_monitor.has_side_areas());
    assert!(initial_area_ids
      .iter()
      .all(|id| state.container_by_id(*id).is_none()));
    for window in [&left_window, &right_window] {
      assert_eq!(
        window.workspace().map(|workspace| workspace.id()),
        Some(selected_workspace.id())
      );
    }
    assert_eq!(
      state.focused_container().map(|container| container.id()),
      Some(left_window.id())
    );
    assert_tree_links_and_focus_order(
      &state.root_container.clone().into(),
    );

    update_mock_monitor(
      &selected_monitor,
      r"\\.\DISPLAY2",
      "DEL439E",
      &mut state,
      &config,
    );

    assert!(selected_monitor.side_area(SideArea::Left).is_some());
    assert!(selected_monitor.side_area(SideArea::Right).is_some());
    assert!(initial_area_ids
      .iter()
      .all(|id| state.container_by_id(*id).is_none()));
    assert_tree_links_and_focus_order(
      &state.root_container.clone().into(),
    );
  }
}
