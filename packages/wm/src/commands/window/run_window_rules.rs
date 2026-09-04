use tracing::info;
use wm_common::{
  InvokeCommand, InvokeMoveCommand, SideArea, WindowRuleConfig,
  WindowRuleEvent,
};
use wm_platform::Direction;

use super::workspace_target_from_move_command;
use crate::{
  models::{Monitor, WindowContainer, Workspace, WorkspaceTarget},
  traits::{CommonGetters, WindowGetters},
  user_config::UserConfig,
  wm::WindowManager,
  wm_state::WmState,
};

/// Returns the window (if it's still attached) after running the window
/// rules.
pub fn run_window_rules(
  window: WindowContainer,
  event_type: &WindowRuleEvent,
  state: &mut WmState,
  config: &mut UserConfig,
) -> anyhow::Result<Option<WindowContainer>> {
  let pending_window_rules =
    config.pending_window_rules(&window, event_type);

  let mut subject_window = window;

  for rule in pending_window_rules {
    if let Some(reason) =
      side_area_rule_defer_reason(&rule, &subject_window, state, config)
    {
      info!(
        "Deferring side-area window rule before executing any commands: \
         {reason}."
      );
      continue;
    }

    info!("Running window rule with commands: {:?}.", rule.commands);

    for command in &rule.commands {
      WindowManager::run_command(
        command,
        subject_window.clone().into(),
        state,
        config,
      )?;

      // Update the subject container in case the container type changes.
      // For example, when going from a tiling to a floating window.
      subject_window = if subject_window.is_detached() {
        match state.window_from_native(&subject_window.native()) {
          Some(window) => window,
          None => return Ok(None),
        }
      } else {
        subject_window
      }
    }

    // Add the window rule as done.
    if rule.run_once {
      let window_rules = subject_window
        .done_window_rules()
        .into_iter()
        .chain(std::iter::once(rule));

      subject_window.set_done_window_rules(window_rules.collect());
    }
  }

  Ok(Some(subject_window))
}

/// Predicted location of a rule's subject window.
#[derive(Clone)]
struct PredictedWindowLocation {
  workspace: Option<Workspace>,
  monitor: Monitor,
  is_side_area: bool,
}

impl PredictedWindowLocation {
  /// Creates a predicted location from an attached workspace.
  fn from_workspace(workspace: Workspace) -> Option<Self> {
    Some(Self {
      monitor: workspace.monitor()?,
      is_side_area: workspace.is_side_area(),
      workspace: Some(workspace),
    })
  }

  /// Creates a predicted location for a workspace that will be activated.
  fn pending_workspace(monitor: Monitor) -> Self {
    Self {
      workspace: None,
      monitor,
      is_side_area: false,
    }
  }

  /// Gets the regular workspace used to resolve a workspace move.
  fn workspace_move_origin(&self) -> Option<Workspace> {
    match (&self.workspace, self.is_side_area) {
      (_, true) => self.monitor.displayed_workspace(),
      (Some(workspace), false) => Some(workspace.clone()),
      (None, false) => None,
    }
  }
}

/// State used while preflighting a side-area window rule.
struct SideAreaRulePreflight<'a> {
  state: &'a WmState,
  config: &'a UserConfig,
  location: Result<PredictedWindowLocation, String>,
  focused_monitor: Option<Monitor>,
  subject_has_focus: Option<bool>,
  workspace_names_are_predictable: bool,
}

impl<'a> SideAreaRulePreflight<'a> {
  /// Starts a preflight from the subject window's current location.
  fn new(
    window: &WindowContainer,
    state: &'a WmState,
    config: &'a UserConfig,
  ) -> Self {
    Self {
      state,
      config,
      location: window
        .workspace()
        .and_then(PredictedWindowLocation::from_workspace)
        .ok_or_else(|| "the window location is unavailable".to_string()),
      focused_monitor: state
        .focused_container()
        .and_then(|focused| focused.monitor()),
      subject_has_focus: Some(window.has_focus(None)),
      workspace_names_are_predictable: true,
    }
  }

  /// Preflights one window `move` command.
  fn move_window(
    &mut self,
    command: &InvokeMoveCommand,
  ) -> Option<String> {
    if command.direction.is_some() {
      self.location = Err(
        "a preceding directional window move is layout-dependent"
          .to_string(),
      );
      self.invalidate_focus();
      return None;
    }

    if let Some(target) = workspace_target_from_move_command(command) {
      if self.workspace_names_are_predictable {
        self.location = self.location.clone().and_then(|current| {
          predict_workspace_move(
            current,
            target,
            self.state,
            self.config,
            self.focused_monitor.as_ref(),
          )
        });
      } else {
        self.location = Err(
          "a preceding workspace rename changes target resolution"
            .to_string(),
        );
      }
      // A workspace move restores focus within the source workspace when
      // the subject was focused. The focused monitor therefore stays the
      // same, but whether the subject still has focus is no longer safe to
      // infer (the move can also be a no-op).
      self.subject_has_focus = None;
    } else if let Some(side) = command.side_area {
      return self.move_window_to_side_area(side);
    }

    None
  }

  /// Preflights a move into the requested side area.
  fn move_window_to_side_area(
    &mut self,
    side: SideArea,
  ) -> Option<String> {
    let predicted = match &self.location {
      Ok(predicted) => predicted,
      Err(reason) => {
        return Some(format!(
          "the monitor at the {side:?} side-area command cannot be \
           predicted because {reason}"
        ));
      }
    };
    let device_name = predicted.monitor.native_properties().device_name;
    let Some(area) = predicted.monitor.side_area(side) else {
      return Some(format!(
        "the {side:?} side area is unavailable on the predicted monitor \
         {device_name:?}"
      ));
    };

    self.location = PredictedWindowLocation::from_workspace(area)
      .ok_or_else(|| "the side area has no monitor".to_string());
    None
  }

  /// Preflights moving the subject's workspace between monitors.
  fn move_workspace(&mut self, direction: &Direction) {
    self.location = self.location.clone().and_then(|mut current| {
      if current.is_side_area {
        return Ok(current);
      }

      let target_monitor = self
        .state
        .monitor_in_direction(&current.monitor, direction)
        .map_err(|err| {
          format!("the workspace destination is unavailable: {err}")
        })?;
      if let Some(target_monitor) = target_monitor {
        current.monitor = target_monitor;
        // The workspace handle still belongs to its current monitor until
        // the real command runs.
        current.workspace = None;
      }
      Ok(current)
    });
    self.update_focus_after_move();
  }

  /// Invalidates predictions that depend on the focused monitor.
  fn invalidate_focus(&mut self) {
    self.focused_monitor = None;
    self.subject_has_focus = None;
  }

  /// Tracks where focus would move when the subject changes monitors.
  fn update_focus_after_move(&mut self) {
    match self.subject_has_focus {
      Some(true) => {
        self.focused_monitor = self
          .location
          .as_ref()
          .ok()
          .map(|location| location.monitor.clone());
      }
      Some(false) => {}
      None => self.focused_monitor = None,
    }
  }
}

/// Returns why a side-area rule must be deferred, if it cannot be
/// executed atomically.
///
/// This preflight follows monitor-changing commands without mutating the
/// tree. If a preceding command makes the destination impossible to
/// predict safely, the whole rule is deferred before its first command.
fn side_area_rule_defer_reason(
  rule: &WindowRuleConfig,
  window: &WindowContainer,
  state: &WmState,
  config: &UserConfig,
) -> Option<String> {
  let has_side_area_move = rule.commands.iter().any(|command| {
    matches!(
      command,
      InvokeCommand::Move(args) if args.side_area.is_some()
    )
  });
  if !has_side_area_move {
    return None;
  }

  let mut preflight = SideAreaRulePreflight::new(window, state, config);

  for command in &rule.commands {
    match command {
      InvokeCommand::Move(args) => {
        if let Some(reason) = preflight.move_window(args) {
          return Some(reason);
        }
      }
      InvokeCommand::MoveWorkspace { direction } => {
        preflight.move_workspace(direction);
      }
      InvokeCommand::UpdateWorkspaceConfig { new_config, .. }
        if new_config.name.is_some() =>
      {
        preflight.workspace_names_are_predictable = false;
      }
      InvokeCommand::Focus(_)
      | InvokeCommand::FocusNextTab
      | InvokeCommand::FocusPreviousTab
      | InvokeCommand::WmCycleFocus { .. } => {
        preflight.invalidate_focus();
      }
      InvokeCommand::WmReloadConfig => {
        preflight.location = Err(
          "a preceding config reload can rebuild monitor side areas"
            .to_string(),
        );
        preflight.invalidate_focus();
      }
      // `Ignore` detaches the subject, so later rule commands are not run.
      InvokeCommand::Ignore => break,
      _ => {}
    }
  }

  None
}

/// Predicts a workspace move without activating or moving containers.
fn predict_workspace_move(
  current: PredictedWindowLocation,
  target: WorkspaceTarget,
  state: &WmState,
  config: &UserConfig,
  focused_monitor: Option<&Monitor>,
) -> Result<PredictedWindowLocation, String> {
  if config.value.workspaces.is_empty()
    && matches!(&target, WorkspaceTarget::Next | WorkspaceTarget::Previous)
  {
    return Err("no workspaces are configured".to_string());
  }

  if let Some(origin_workspace) = current.workspace_move_origin() {
    let (target_name, target_workspace) = state
      .workspace_by_target(&origin_workspace, target, config)
      .map_err(|err| {
        format!("workspace target resolution failed: {err}")
      })?;

    if let Some(target_workspace) = target_workspace {
      return PredictedWindowLocation::from_workspace(target_workspace)
        .ok_or_else(|| "the target workspace has no monitor".to_string());
    }

    return match target_name {
      Some(name) => {
        predict_named_workspace(&name, state, config, focused_monitor)
      }
      None => Ok(current),
    };
  }

  Err(
    "a workspace target follows a workspace that is not yet attached"
      .to_string(),
  )
}

/// Predicts the location of a named active or inactive workspace.
fn predict_named_workspace(
  name: &str,
  state: &WmState,
  config: &UserConfig,
  focused_monitor: Option<&Monitor>,
) -> Result<PredictedWindowLocation, String> {
  if let Some(workspace) = state.workspace_by_name(name) {
    return PredictedWindowLocation::from_workspace(workspace)
      .ok_or_else(|| "the target workspace has no monitor".to_string());
  }

  let workspace_config = config
    .value
    .workspaces
    .iter()
    .find(|workspace| workspace.name == name)
    .ok_or_else(|| format!("workspace {name:?} is not configured"))?;
  let monitor = workspace_config
    .bind_to_monitor
    .and_then(|monitor_index| {
      state
        .monitors()
        .into_iter()
        .find(|monitor| monitor.index() == monitor_index as usize)
    })
    .or_else(|| focused_monitor.cloned())
    .ok_or_else(|| {
      format!(
        "the activation monitor for workspace {name:?} cannot be predicted"
      )
    })?;

  Ok(PredictedWindowLocation::pending_workspace(monitor))
}

#[cfg(test)]
mod tests {
  use wm_common::{MatchType, MonitorMatchConfig, ParsedConfig, SideArea};

  use super::*;
  use crate::{
    commands::monitor::ensure_side_areas,
    models::{Monitor, TilingWindow, Workspace},
    test_utils::state_with_monitors,
  };

  fn assert_manage_rule_respects_monitor_selector(side: SideArea) {
    let target_window = TilingWindow::mock()
      .process_name("widget".to_string())
      .call();
    let target_workspace = Workspace::mock()
      .tiling_containers(vec![target_window.clone().into()])
      .call();
    let target_monitor = Monitor::mock()
      .device_name("DISPLAY1".to_string())
      .workspaces(vec![target_workspace])
      .call();

    let other_window = TilingWindow::mock()
      .process_name("widget".to_string())
      .call();
    let other_workspace = Workspace::mock()
      .tiling_containers(vec![other_window.clone().into()])
      .call();
    let other_monitor = Monitor::mock()
      .device_name("DISPLAY2".to_string())
      .workspaces(vec![other_workspace.clone()])
      .call();
    let mut state = state_with_monitors(vec![
      target_monitor.clone(),
      other_monitor.clone(),
    ]);
    let side_name = match side {
      SideArea::Left => "left",
      SideArea::Right => "right",
    };
    let parsed_config = serde_yaml::from_str::<ParsedConfig>(&format!(
      r"
side_areas:
  left: 300px
  right: 300px
  match:
    - device_name: {{ equals: DISPLAY1 }}
window_rules:
  - commands: ['move --side-area {side_name}']
    match:
      - window_process: {{ equals: widget }}
workspaces:
  - name: '1'
"
    ))
    .unwrap();
    let mut config = UserConfig::mock_with_value(parsed_config);

    ensure_side_areas(&target_monitor, &state, &config).unwrap();
    ensure_side_areas(&other_monitor, &state, &config).unwrap();

    run_window_rules(
      target_window.clone().into(),
      &WindowRuleEvent::Manage,
      &mut state,
      &mut config,
    )
    .unwrap();
    run_window_rules(
      other_window.clone().into(),
      &WindowRuleEvent::Manage,
      &mut state,
      &mut config,
    )
    .unwrap();

    assert_eq!(
      target_window
        .workspace()
        .and_then(|workspace| workspace.side_area()),
      Some(side)
    );
    assert_eq!(target_window.done_window_rules().len(), 1);
    assert_eq!(
      other_window.workspace().map(|workspace| workspace.id()),
      Some(other_workspace.id())
    );
    assert_eq!(other_window.done_window_rules().len(), 0);
    assert!(other_monitor.side_area(SideArea::Left).is_none());
    assert!(other_monitor.side_area(SideArea::Right).is_none());

    config.value.side_areas.match_monitor =
      Some(vec![MonitorMatchConfig {
        device_name: MatchType::Equals {
          equals: "DISPLAY2".to_string(),
        },
      }]);
    ensure_side_areas(&target_monitor, &state, &config).unwrap();
    ensure_side_areas(&other_monitor, &state, &config).unwrap();
    run_window_rules(
      other_window.clone().into(),
      &WindowRuleEvent::Manage,
      &mut state,
      &mut config,
    )
    .unwrap();

    assert_eq!(
      other_window
        .workspace()
        .and_then(|workspace| workspace.side_area()),
      Some(side)
    );
    assert_eq!(other_window.done_window_rules().len(), 1);
  }

  fn assert_multi_command_rule_preflight(
    side: SideArea,
    starts_on_selected_monitor: bool,
  ) {
    let window = TilingWindow::mock()
      .process_name("widget".to_string())
      .call();
    let selected_workspace = Workspace::mock()
      .name("1".to_string())
      .tiling_containers(if starts_on_selected_monitor {
        vec![window.clone().into()]
      } else {
        Vec::new()
      })
      .call();
    let selected_monitor = Monitor::mock()
      .device_name("DISPLAY1".to_string())
      .workspaces(vec![selected_workspace.clone()])
      .call();
    let other_workspace = Workspace::mock()
      .name("2".to_string())
      .tiling_containers(if starts_on_selected_monitor {
        Vec::new()
      } else {
        vec![window.clone().into()]
      })
      .call();
    let other_monitor = Monitor::mock()
      .device_name("DISPLAY2".to_string())
      .workspaces(vec![other_workspace.clone()])
      .call();
    let mut state = state_with_monitors(vec![
      selected_monitor.clone(),
      other_monitor.clone(),
    ]);
    let side_name = match side {
      SideArea::Left => "left",
      SideArea::Right => "right",
    };
    let target_workspace_name =
      if starts_on_selected_monitor { "2" } else { "1" };
    let parsed_config = serde_yaml::from_str::<ParsedConfig>(&format!(
      r"
side_areas:
  left: 300px
  right: 300px
  match:
    - device_name: {{ equals: DISPLAY1 }}
window_rules:
  - commands:
      - 'move --workspace {target_workspace_name}'
      - 'move --side-area {side_name}'
    match:
      - window_process: {{ equals: widget }}
workspaces:
  - name: '1'
  - name: '2'
"
    ))
    .unwrap();
    let mut config = UserConfig::mock_with_value(parsed_config);

    ensure_side_areas(&selected_monitor, &state, &config).unwrap();
    ensure_side_areas(&other_monitor, &state, &config).unwrap();

    let result = run_window_rules(
      window.clone().into(),
      &WindowRuleEvent::Manage,
      &mut state,
      &mut config,
    );
    assert!(
      result.is_ok(),
      "rule failed after moving the window to workspace {:?}: {:?}",
      window.workspace().map(|workspace| workspace.config().name),
      result.err()
    );

    if starts_on_selected_monitor {
      assert_eq!(
        window.workspace().map(|workspace| workspace.id()),
        Some(selected_workspace.id())
      );
      assert_eq!(window.done_window_rules().len(), 0);
    } else {
      assert_eq!(
        window
          .workspace()
          .and_then(|workspace| workspace.side_area()),
        Some(side)
      );
      assert_eq!(window.done_window_rules().len(), 1);
    }

    assert!(selected_monitor.side_area(side).is_some());
    assert!(other_monitor.side_area(side).is_none());
  }

  #[test]
  fn manage_side_area_rule_skips_nonmatching_monitors_for_both_sides() {
    assert_manage_rule_respects_monitor_selector(SideArea::Left);
    assert_manage_rule_respects_monitor_selector(SideArea::Right);
  }

  #[test]
  fn manage_preflights_workspace_moves_before_side_area_for_both_sides() {
    for side in [SideArea::Left, SideArea::Right] {
      assert_multi_command_rule_preflight(side, true);
      assert_multi_command_rule_preflight(side, false);
    }
  }
}
