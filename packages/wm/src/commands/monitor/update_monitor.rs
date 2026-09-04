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
    commands::container::attach_container, models::Workspace,
    user_config::UserConfig,
  };

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
        device_name: wm_common::MatchType::Equals {
          equals: "DISPLAY1".to_string(),
        },
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
}
