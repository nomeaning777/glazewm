use anyhow::Context;
use tracing::{info, warn};
#[cfg(target_os = "windows")]
use wm_common::{HideMethod, ParsedConfig};
use wm_common::{WindowRuleEvent, WmEvent};
#[cfg(target_os = "windows")]
use wm_platform::NativeWindowWindowsExt;

use crate::{
  commands::{
    monitor::ensure_side_areas, window::run_window_rules,
    workspace::sort_workspaces,
  },
  traits::{CommonGetters, TilingSizeGetters, WindowGetters},
  user_config::UserConfig,
  wm::WindowManager,
  wm_state::WmState,
};

pub fn reload_config(
  state: &mut WmState,
  config: &mut UserConfig,
) -> anyhow::Result<()> {
  info!("Config reloaded.");

  // Keep reference to old config for comparison.
  #[cfg(target_os = "windows")]
  let old_config = config.value.clone();

  // Re-evaluate user config file and set its values in state.
  config.reload()?;

  // Apply side-area changes before window rules, since a rule can move a
  // window into an area that was enabled by this reload.
  for monitor in state.monitors() {
    ensure_side_areas(&monitor, state, config)?;
  }

  // Re-run window rules on all active windows.
  for window in state.windows() {
    window.set_done_window_rules(Vec::new());
    run_window_rules(window, &WindowRuleEvent::Manage, state, config)?;
  }

  update_workspace_configs(state, config)?;

  update_container_gaps(state, config);

  #[cfg(target_os = "windows")]
  update_window_effects(&old_config, state, config)?;

  // Ensure all windows are shown when hide method is changed.
  #[cfg(target_os = "windows")]
  if old_config.general.hide_method != config.value.general.hide_method
    && config.value.general.hide_method == HideMethod::Cloak
  {
    for window in state.windows() {
      let _ = window.native().show();
    }
  }

  // Ensure all windows are shown in taskbar when `show_all_in_taskbar` is
  // changed.
  #[cfg(target_os = "windows")]
  if old_config.general.show_all_in_taskbar
    != config.value.general.show_all_in_taskbar
    && config.value.general.show_all_in_taskbar
  {
    for window in state.windows() {
      let _ = window.native().set_taskbar_visibility(true);
    }
  }

  // Clear active binding modes.
  state.binding_modes = Vec::new();

  // Redraw full container tree.
  state
    .pending_sync
    .queue_container_to_redraw(state.root_container.clone());

  // Emit the updated config.
  state.emit_event(WmEvent::UserConfigChanged {
    config_path: config
      .path
      .to_str()
      .context("Invalid config path.")?
      .to_string(),
    config_string: config.value_str.clone(),
    parsed_config: config.value.clone(),
  });

  // Run config reload commands.
  WindowManager::run_commands(
    &config.value.general.config_reload_commands.clone(),
    state.focused_container().context("No focused container.")?,
    state,
    config,
  )?;

  Ok(())
}

/// Update configs of active workspaces.
fn update_workspace_configs(
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let workspaces = state.workspaces();

  for workspace in &workspaces {
    let monitor = workspace.monitor().context("No monitor.")?;

    let workspace_config = config
      .value
      .workspaces
      .iter()
      .find(|config| config.name == workspace.config().name)
      .or_else(|| {
        // When the workspace config is not found, the current name of the
        // workspace has been removed. So, we reassign the first suitable
        // workspace config to the workspace.
        config
          .workspace_config_for_monitor(&monitor, &workspaces)
          .or_else(|| config.next_inactive_workspace_config(&workspaces))
      });

    match workspace_config {
      None => {
        warn!(
          "Unable to update workspace config. No available workspace configs."
        );
      }
      Some(workspace_config) => {
        if *workspace_config != workspace.config() {
          workspace.set_config(workspace_config.clone());

          sort_workspaces(&monitor, config)?;

          state.emit_event(WmEvent::WorkspaceUpdated {
            updated_workspace: workspace.to_dto()?,
          });
        }
      }
    }
  }

  Ok(())
}

/// Updates outer gap of workspaces and inner gaps of tiling containers.
fn update_container_gaps(state: &mut WmState, config: &UserConfig) {
  let tiling_containers = state
    .root_container
    .self_and_descendants()
    .filter_map(|container| container.as_tiling_container().ok());

  for container in tiling_containers {
    container.set_gaps_config(config.value.gaps.clone());
  }

  for workspace in state.workspaces() {
    workspace.set_gaps_config(config.value.gaps.clone());
  }
}

#[cfg(target_os = "windows")]
fn update_window_effects(
  old_config: &ParsedConfig,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let focused_container =
    state.focused_container().context("No focused container.")?;

  let window_effects = &config.value.window_effects;
  let old_window_effects = &old_config.window_effects;

  // Window border effects are left at system defaults if disabled in the
  // config. However, when transitioning from colored borders to having
  // them disabled, it's best to reset to the system defaults.
  if !window_effects.focused_window.border.enabled
    && old_window_effects.focused_window.border.enabled
  {
    if let Ok(window) = focused_container.as_window_container() {
      _ = window.native().set_border_color(None);
    }
  }

  if !window_effects.other_windows.border.enabled
    && old_window_effects.other_windows.border.enabled
  {
    let unfocused_windows = state
      .windows()
      .into_iter()
      .filter(|window| window.id() != focused_container.id());

    for window in unfocused_windows {
      _ = window.native().set_border_color(None);
    }
  }

  state.pending_sync.queue_all_effects_update();

  Ok(())
}

#[cfg(test)]
mod tests {
  use std::fs;

  use tokio::sync::mpsc;
  use uuid::Uuid;
  use wm_common::SideArea;
  use wm_platform::Dispatcher;

  use super::*;
  use crate::{
    commands::container::{attach_container, set_focused_descendant},
    models::{Monitor, TilingWindow, WindowContainer, Workspace},
    traits::{CommonGetters, PositionGetters, WindowGetters},
  };

  fn reload_test_config(
    selected_monitor: &str,
    display_name: &str,
    outer_gap: i32,
  ) -> String {
    format!(
      r"
side_areas:
  left: 240px
  right: 260px
  match:
    - device_name: {{ equals: {selected_monitor} }}
gaps:
  scale_with_dpi: false
  inner_gap: 7px
  outer_gap:
    top: {outer_gap}px
    right: {outer_gap}px
    bottom: {outer_gap}px
    left: {outer_gap}px
window_effects:
  focused_window:
    border:
      enabled: true
window_rules:
  - commands: ['move --side-area left']
    match:
      - window_process: {{ equals: left-widget }}
  - commands: ['move --side-area right']
    match:
      - window_process: {{ equals: right-widget }}
workspaces:
  - name: '1'
    display_name: {display_name}
"
    )
  }

  fn assert_window_rule_state(
    window: &WindowContainer,
    expected_side: Option<SideArea>,
    regular_workspace: &Workspace,
  ) {
    if let Some(side) = expected_side {
      assert_eq!(
        window
          .workspace()
          .and_then(|workspace| workspace.side_area()),
        Some(side)
      );
      assert_eq!(window.done_window_rules().len(), 1);
    } else {
      assert_eq!(
        window.workspace().map(|workspace| workspace.id()),
        Some(regular_workspace.id())
      );
      assert_eq!(window.done_window_rules().len(), 0);
    }
  }

  fn assert_monitor_side_areas(monitor: &Monitor, enabled: bool) {
    assert_eq!(monitor.side_area(SideArea::Left).is_some(), enabled);
    assert_eq!(monitor.side_area(SideArea::Right).is_some(), enabled);
  }

  struct RuleMonitorFixture {
    monitor: Monitor,
    workspace: Workspace,
    left_window: TilingWindow,
    right_window: TilingWindow,
  }

  impl RuleMonitorFixture {
    fn new(device_name: &str) -> Self {
      let left_window = TilingWindow::mock()
        .process_name("left-widget".to_string())
        .call();
      let right_window = TilingWindow::mock()
        .process_name("right-widget".to_string())
        .call();
      let workspace = Workspace::mock()
        .tiling_containers(vec![
          left_window.clone().into(),
          right_window.clone().into(),
        ])
        .call();
      let monitor = Monitor::mock()
        .device_name(device_name.to_string())
        .workspaces(vec![workspace.clone()])
        .call();

      Self {
        monitor,
        workspace,
        left_window,
        right_window,
      }
    }

    fn assert_rule_state(&self, selected: bool) {
      assert_monitor_side_areas(&self.monitor, selected);
      assert_window_rule_state(
        &self.left_window.clone().into(),
        selected.then_some(SideArea::Left),
        &self.workspace,
      );
      assert_window_rule_state(
        &self.right_window.clone().into(),
        selected.then_some(SideArea::Right),
        &self.workspace,
      );
    }
  }

  struct ReloadRulesFixture {
    state: WmState,
    config: UserConfig,
    config_path: std::path::PathBuf,
    monitors: [RuleMonitorFixture; 2],
  }

  impl ReloadRulesFixture {
    fn new() -> Self {
      let monitors = [
        RuleMonitorFixture::new("DISPLAY1"),
        RuleMonitorFixture::new("DISPLAY2"),
      ];
      let (event_tx, _event_rx) = mpsc::unbounded_channel();
      let (exit_tx, _exit_rx) = mpsc::unbounded_channel();
      let state = WmState::new(Dispatcher::mock(), event_tx, exit_tx);
      for fixture in &monitors {
        attach_container(
          &fixture.monitor.clone().into(),
          &state.root_container.clone().into(),
          None,
        )
        .unwrap();
      }
      set_focused_descendant(
        &monitors[0].left_window.clone().into(),
        None,
      );

      let config_path = std::env::temp_dir().join(format!(
        "glazewm-reload-rules-test-{}.yaml",
        Uuid::new_v4()
      ));
      let mut config = UserConfig::mock();
      config.path = config_path.clone();

      Self {
        state,
        config,
        config_path,
        monitors,
      }
    }

    fn reload(
      &mut self,
      selected_monitor: &str,
      display_name: &str,
      outer_gap: i32,
    ) {
      self.state.pending_sync.clear();
      fs::write(
        &self.config_path,
        reload_test_config(selected_monitor, display_name, outer_gap),
      )
      .unwrap();
      reload_config(&mut self.state, &mut self.config).unwrap();
    }

    fn assert_selected(&self, selected_index: usize) {
      for (index, fixture) in self.monitors.iter().enumerate() {
        fixture.assert_rule_state(index == selected_index);
      }
    }

    fn assert_late_reload_updates(
      &self,
      display_name: &str,
      outer_gap: f32,
    ) {
      for fixture in &self.monitors {
        assert_eq!(
          fixture.workspace.config().display_name.as_deref(),
          Some(display_name)
        );
        assert_eq!(fixture.workspace.outer_gaps().top.amount, outer_gap);
        assert_eq!(fixture.workspace.outer_gaps().right.amount, outer_gap);
      }
      assert!(
        self
          .config
          .value
          .window_effects
          .focused_window
          .border
          .enabled
      );
      #[cfg(target_os = "windows")]
      assert!(self.state.pending_sync.needs_all_effects_update());
    }

    fn area_ids(&self, monitor_index: usize) -> [Uuid; 2] {
      let monitor = &self.monitors[monitor_index].monitor;
      [
        monitor.side_area(SideArea::Left).unwrap().id(),
        monitor.side_area(SideArea::Right).unwrap().id(),
      ]
    }

    fn cleanup(&self) {
      fs::remove_file(&self.config_path).unwrap();
    }
  }

  #[test]
  fn reload_removes_side_areas_from_nonmatching_monitor() {
    let side_area = Workspace::mock_side_area().call();
    let workspace = Workspace::mock().call();
    let monitor = Monitor::mock()
      .device_name("DISPLAY2".to_string())
      .workspaces(vec![side_area, workspace.clone()])
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
    set_focused_descendant(&workspace.clone().into(), None);

    let config_path = std::env::temp_dir()
      .join(format!("glazewm-reload-test-{}.yaml", Uuid::new_v4()));
    fs::write(
      &config_path,
      r"
side_areas:
  left: 300px
  match:
    - device_name: { equals: DISPLAY1 }
workspaces:
  - name: '1'
",
    )
    .unwrap();
    let mut config = UserConfig::mock();
    config.path = config_path.clone();

    let result = reload_config(&mut state, &mut config);
    fs::remove_file(config_path).unwrap();
    result.unwrap();

    assert!(monitor.side_area(SideArea::Left).is_none());
    assert_eq!(workspace.to_rect().unwrap().width(), 1680);
  }

  #[test]
  fn reload_moves_side_area_rules_between_selected_monitors_consistently()
  {
    let mut fixture = ReloadRulesFixture::new();

    fixture.reload("DISPLAY1", "first", 11);
    fixture.assert_selected(0);

    fixture.reload("DISPLAY2", "second", 17);
    fixture.assert_selected(1);
    fixture.assert_late_reload_updates("second", 17.0);

    let second_area_ids = fixture.area_ids(1);
    fixture.reload("DISPLAY2", "second", 17);
    fixture.assert_selected(1);
    assert_eq!(fixture.area_ids(1), second_area_ids);
    fixture.assert_late_reload_updates("second", 17.0);

    fixture.reload("DISPLAY1", "third", 23);
    fixture.assert_selected(0);
    fixture.assert_late_reload_updates("third", 23.0);

    fixture.cleanup();
  }
}
