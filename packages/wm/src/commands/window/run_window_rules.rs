use tracing::info;
use wm_common::{
  InvokeCommand, InvokeFocusCommand, InvokeMoveCommand,
  InvokeUpdateWorkspaceConfig, SideArea, WindowRuleConfig,
  WindowRuleEvent,
};
use wm_platform::Direction;

use super::workspace_target_from_move_command;
use crate::{
  commands::workspace::workspace_target_from_focus_command,
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
  workspace_name: Option<String>,
  monitor: Monitor,
}

impl PredictedWindowLocation {
  /// Creates a predicted location from an attached workspace.
  fn from_workspace(workspace: &Workspace) -> Option<Self> {
    Some(Self {
      monitor: workspace.monitor()?,
      workspace_name: (!workspace.is_side_area())
        .then(|| workspace.config().name),
    })
  }

  /// Creates a predicted location in a regular workspace.
  fn regular_workspace(name: String, monitor: Monitor) -> Self {
    Self {
      workspace_name: Some(name),
      monitor,
    }
  }

  /// Gets the regular workspace used to resolve a workspace move.
  fn workspace_move_origin(
    &self,
    state: &PredictedWorkspaceState,
  ) -> Result<String, String> {
    self
      .workspace_name
      .clone()
      .map_or_else(|| state.displayed_workspace_name(&self.monitor), Ok)
  }
}

/// Workspace data that can affect target resolution during preflight.
#[derive(Clone)]
struct PredictedWorkspace {
  name: String,
  monitor: Monitor,
  has_children: bool,
  keep_alive: bool,
}

/// Read-only simulation of workspace focus and activation state.
struct PredictedWorkspaceState<'a> {
  state: &'a WmState,
  config: &'a UserConfig,
  workspaces: Vec<PredictedWorkspace>,
  displayed_workspaces: Vec<(Monitor, String)>,
  focused_workspace: Option<String>,
  recent_workspace: Option<String>,
  can_focus_workspace: bool,
}

impl<'a> PredictedWorkspaceState<'a> {
  /// Takes a snapshot of state used by workspace target resolution.
  fn new(
    state: &'a WmState,
    config: &'a UserConfig,
  ) -> Result<Self, String> {
    let workspaces = state
      .workspaces()
      .into_iter()
      .map(|workspace| {
        let workspace_config = workspace.config();
        Ok(PredictedWorkspace {
          name: workspace_config.name,
          monitor: workspace.monitor().ok_or_else(|| {
            "an active workspace has no monitor".to_string()
          })?,
          has_children: workspace.has_children(),
          keep_alive: workspace_config.keep_alive,
        })
      })
      .collect::<Result<Vec<_>, String>>()?;
    let displayed_workspaces = state
      .monitors()
      .into_iter()
      .filter_map(|monitor| {
        let name = monitor.displayed_workspace()?.config().name;
        Some((monitor, name))
      })
      .collect();
    let focused_workspace = state
      .focused_container()
      .and_then(|focused| focused.workspace())
      .and_then(|workspace| {
        if workspace.is_side_area() {
          workspace
            .monitor()
            .and_then(|monitor| monitor.displayed_workspace())
            .map(|workspace| workspace.config().name)
        } else {
          Some(workspace.config().name)
        }
      });

    Ok(Self {
      state,
      config,
      workspaces,
      displayed_workspaces,
      focused_workspace,
      recent_workspace: state.recent_workspace_name.clone(),
      can_focus_workspace: true,
    })
  }

  /// Gets an active workspace by name.
  fn workspace_by_name(&self, name: &str) -> Option<&PredictedWorkspace> {
    self
      .workspaces
      .iter()
      .find(|workspace| workspace.name == name)
  }

  /// Applies config fields that affect workspace target resolution.
  fn update_workspace_config(
    &mut self,
    name: &str,
    new_config: &InvokeUpdateWorkspaceConfig,
  ) -> Result<bool, String> {
    let renamed = new_config
      .name
      .as_ref()
      .is_some_and(|new_name| new_name != name);
    if renamed
      && new_config
        .name
        .as_ref()
        .is_some_and(|new_name| self.workspace_by_name(new_name).is_some())
    {
      return Err("the updated workspace name already exists".to_string());
    }

    let workspace = self
      .workspaces
      .iter_mut()
      .find(|workspace| workspace.name == name)
      .ok_or_else(|| format!("active workspace {name:?} was not found"))?;
    if let Some(keep_alive) = new_config.keep_alive {
      workspace.keep_alive = keep_alive;
    }

    Ok(renamed)
  }

  /// Gets the predicted displayed workspace on a monitor.
  fn displayed_workspace_name(
    &self,
    monitor: &Monitor,
  ) -> Result<String, String> {
    self
      .displayed_workspaces
      .iter()
      .find(|(candidate, _)| candidate.id() == monitor.id())
      .map(|(_, name)| name.clone())
      .ok_or_else(|| {
        format!(
          "monitor {:?} has no predicted displayed workspace",
          monitor.native_properties().device_name
        )
      })
  }

  /// Sets the predicted displayed workspace on a monitor.
  fn set_displayed_workspace(&mut self, monitor: &Monitor, name: &str) {
    if let Some((_, displayed_name)) = self
      .displayed_workspaces
      .iter_mut()
      .find(|(candidate, _)| candidate.id() == monitor.id())
    {
      *displayed_name = name.to_string();
    } else {
      self
        .displayed_workspaces
        .push((monitor.clone(), name.to_string()));
    }
  }

  /// Gets names of active workspaces in config order.
  fn sorted_workspace_names(
    &self,
    monitor: Option<&Monitor>,
  ) -> Vec<String> {
    let mut workspaces = self
      .workspaces
      .iter()
      .filter(|workspace| {
        monitor
          .is_none_or(|monitor| workspace.monitor.id() == monitor.id())
      })
      .collect::<Vec<_>>();
    workspaces.sort_by_key(|workspace| {
      self.config.workspace_config_index(&workspace.name)
    });
    workspaces
      .into_iter()
      .map(|workspace| workspace.name.clone())
      .collect()
  }

  /// Inserts a workspace in the order produced by `sort_workspaces`.
  fn insert_workspace(&mut self, workspace: PredictedWorkspace) {
    let monitor_id = workspace.monitor.id();
    let insertion_index = self
      .workspaces
      .iter()
      .position(|candidate| candidate.monitor.id() == monitor_id)
      .unwrap_or_else(|| {
        let monitor_ids = self
          .state
          .monitors()
          .into_iter()
          .map(|monitor| monitor.id())
          .collect::<Vec<_>>();
        let monitor_index = monitor_ids
          .iter()
          .position(|id| *id == monitor_id)
          .unwrap_or(monitor_ids.len());
        self
          .workspaces
          .iter()
          .position(|candidate| {
            monitor_ids
              .iter()
              .position(|id| *id == candidate.monitor.id())
              .is_some_and(|index| index > monitor_index)
          })
          .unwrap_or(self.workspaces.len())
      });
    let mut monitor_workspaces = self
      .workspaces
      .iter()
      .filter(|candidate| candidate.monitor.id() == monitor_id)
      .cloned()
      .chain(std::iter::once(workspace))
      .collect::<Vec<_>>();
    monitor_workspaces.sort_by_key(|candidate| {
      self.config.workspace_config_index(&candidate.name)
    });

    self
      .workspaces
      .retain(|candidate| candidate.monitor.id() != monitor_id);
    self
      .workspaces
      .splice(insertion_index..insertion_index, monitor_workspaces);
  }

  /// Gets the adjacent active workspace, with wrapping.
  fn adjacent_active_workspace(
    &self,
    origin_name: &str,
    monitor: Option<&Monitor>,
    next: bool,
  ) -> Result<Option<String>, String> {
    let names = self.sorted_workspace_names(monitor);
    let origin_index = names
      .iter()
      .position(|name| name == origin_name)
      .ok_or_else(|| {
      format!("active workspace {origin_name:?} was not found")
    })?;
    let target = if next {
      names.get(origin_index + 1).or_else(|| names.first())
    } else if origin_index == 0 {
      names.last()
    } else {
      names.get(origin_index - 1)
    };
    Ok(target.cloned())
  }

  /// Gets the adjacent configured workspace, with wrapping.
  fn adjacent_configured_workspace(
    &self,
    origin_name: &str,
    next: bool,
  ) -> Result<Option<String>, String> {
    let workspaces = &self.config.value.workspaces;
    let origin_index = workspaces
      .iter()
      .position(|workspace| workspace.name == origin_name)
      .ok_or_else(|| {
        format!("configured workspace {origin_name:?} was not found")
      })?;
    let target = if next {
      workspaces
        .get(origin_index + 1)
        .or_else(|| workspaces.first())
    } else if origin_index == 0 {
      workspaces.last()
    } else {
      workspaces.get(origin_index - 1)
    };
    Ok(target.map(|workspace| workspace.name.clone()))
  }

  /// Resolves a target using the predicted workspace state.
  fn resolve_workspace_target(
    &self,
    origin_name: &str,
    target: WorkspaceTarget,
  ) -> Result<Option<String>, String> {
    match target {
      WorkspaceTarget::Name(name) => {
        if origin_name != name {
          Ok(Some(name))
        } else if self.config.value.general.toggle_workspace_on_refocus {
          Ok(self.recent_workspace.clone())
        } else {
          Ok(None)
        }
      }
      WorkspaceTarget::Recent => Ok(self.recent_workspace.clone()),
      WorkspaceTarget::NextActive => {
        self.adjacent_active_workspace(origin_name, None, true)
      }
      WorkspaceTarget::PreviousActive => {
        self.adjacent_active_workspace(origin_name, None, false)
      }
      WorkspaceTarget::NextActiveInMonitor
      | WorkspaceTarget::PreviousActiveInMonitor => {
        let monitor = self
          .workspace_by_name(origin_name)
          .map(|workspace| workspace.monitor.clone())
          .ok_or_else(|| {
            format!("active workspace {origin_name:?} was not found")
          })?;
        self.adjacent_active_workspace(
          origin_name,
          Some(&monitor),
          matches!(target, WorkspaceTarget::NextActiveInMonitor),
        )
      }
      WorkspaceTarget::Next => {
        self.adjacent_configured_workspace(origin_name, true)
      }
      WorkspaceTarget::Previous => {
        self.adjacent_configured_workspace(origin_name, false)
      }
      WorkspaceTarget::Direction(direction) => {
        let origin_monitor = self
          .workspace_by_name(origin_name)
          .map(|workspace| workspace.monitor.clone())
          .ok_or_else(|| {
            format!("active workspace {origin_name:?} was not found")
          })?;
        self
          .state
          .monitor_in_direction(&origin_monitor, &direction)
          .map_err(|err| {
            format!("workspace direction resolution failed: {err}")
          })?
          .map(|monitor| self.displayed_workspace_name(&monitor))
          .transpose()
      }
    }
  }

  /// Activates a named workspace in the predicted state if necessary.
  fn ensure_workspace_active(
    &mut self,
    name: &str,
  ) -> Result<Monitor, String> {
    if let Some(workspace) = self.workspace_by_name(name) {
      return Ok(workspace.monitor.clone());
    }

    let workspace_config = self
      .config
      .value
      .workspaces
      .iter()
      .find(|workspace| workspace.name == name)
      .cloned()
      .ok_or_else(|| format!("workspace {name:?} is not configured"))?;
    let monitor = workspace_config
      .bind_to_monitor
      .and_then(|monitor_index| {
        self
          .state
          .monitors()
          .into_iter()
          .find(|monitor| monitor.index() == monitor_index as usize)
      })
      .or_else(|| {
        self.focused_workspace.as_ref().and_then(|focused_name| {
          self
            .workspace_by_name(focused_name)
            .map(|workspace| workspace.monitor.clone())
        })
      })
      .ok_or_else(|| {
        format!(
          "the activation monitor for workspace {name:?} cannot be predicted"
        )
      })?;

    self.insert_workspace(PredictedWorkspace {
      name: name.to_string(),
      monitor: monitor.clone(),
      has_children: false,
      keep_alive: workspace_config.keep_alive,
    });
    if self.displayed_workspace_name(&monitor).is_err() {
      self.set_displayed_workspace(&monitor, name);
    }
    Ok(monitor)
  }

  /// Applies the workspace-affecting state changes of `focus_workspace`.
  fn focus_workspace(
    &mut self,
    target: WorkspaceTarget,
  ) -> Result<(), String> {
    if !self.can_focus_workspace {
      return Err(
        "a preceding window move makes empty-workspace deactivation \
         unpredictable"
          .to_string(),
      );
    }

    let origin_name = self.focused_workspace.clone().ok_or_else(|| {
      "no workspace is predicted to have focus".to_string()
    })?;
    let Some(target_name) =
      self.resolve_workspace_target(&origin_name, target)?
    else {
      return Ok(());
    };
    let target_monitor = self.ensure_workspace_active(&target_name)?;

    self.set_displayed_workspace(&target_monitor, &target_name);
    self.focused_workspace = Some(target_name);
    self.deactivate_first_empty_hidden_workspace();
    self.recent_workspace = Some(origin_name);
    Ok(())
  }

  /// Applies the workspace-affecting state changes of `focus_monitor`.
  fn focus_monitor(&mut self, monitor_index: usize) -> Result<(), String> {
    let monitor = self
      .state
      .monitors()
      .get(monitor_index)
      .cloned()
      .ok_or_else(|| {
        format!("monitor at index {monitor_index} was not found")
      })?;
    let workspace_name = self.displayed_workspace_name(&monitor)?;
    self.focus_workspace(WorkspaceTarget::Name(workspace_name))
  }

  /// Removes the first empty, hidden, non-persistent workspace.
  fn deactivate_first_empty_hidden_workspace(&mut self) {
    let workspace_to_remove = self
      .workspaces
      .iter()
      .find(|workspace| {
        !workspace.keep_alive
          && !workspace.has_children
          && !self
            .displayed_workspaces
            .iter()
            .any(|(_, displayed_name)| displayed_name == &workspace.name)
      })
      .map(|workspace| workspace.name.clone());

    if let Some(name) = workspace_to_remove {
      self.workspaces.retain(|workspace| workspace.name != name);
    }
  }
}

/// State used while preflighting a side-area window rule.
struct SideAreaRulePreflight<'a> {
  state: &'a WmState,
  location: Result<PredictedWindowLocation, String>,
  workspace_state: Result<PredictedWorkspaceState<'a>, String>,
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
      location: window
        .workspace()
        .and_then(|workspace| {
          PredictedWindowLocation::from_workspace(&workspace)
        })
        .ok_or_else(|| "the window location is unavailable".to_string()),
      workspace_state: PredictedWorkspaceState::new(state, config),
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
      self.workspace_state = Err(
        "a preceding directional window move changes focus or workspace \
         state unpredictably"
          .to_string(),
      );
      return None;
    }

    if let Some(target) = workspace_target_from_move_command(command) {
      self.location =
        match (self.location.clone(), self.workspace_state.as_mut()) {
          (Ok(current), Ok(workspace_state)) => {
            predict_workspace_move(current, target, workspace_state)
          }
          (Err(reason), _) => Err(reason),
          (_, Err(reason)) => Err(reason.clone()),
        };
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

    self.location = PredictedWindowLocation::from_workspace(&area)
      .ok_or_else(|| "the side area has no monitor".to_string());
    if let Ok(workspace_state) = &mut self.workspace_state {
      workspace_state.can_focus_workspace = false;
    }
    None
  }

  /// Preflights a `focus` command and its workspace state changes.
  fn focus(&mut self, command: &InvokeFocusCommand) {
    let result = match &mut self.workspace_state {
      Ok(workspace_state) => {
        if let Some(target) = workspace_target_from_focus_command(command)
        {
          workspace_state.focus_workspace(target)
        } else if let Some(monitor_index) = command.monitor {
          workspace_state.focus_monitor(monitor_index)
        } else {
          Err(
            "a preceding non-workspace focus command changes focus \
             unpredictably"
              .to_string(),
          )
        }
      }
      Err(_) => return,
    };
    if let Err(reason) = result {
      self.workspace_state = Err(reason);
    }
  }

  /// Preflights the workspace state changed by a config update.
  fn update_workspace_config(
    &mut self,
    workspace_name: Option<&str>,
    new_config: &InvokeUpdateWorkspaceConfig,
  ) -> Option<String> {
    let target_name = match workspace_name {
      Some(name) => name.to_string(),
      None => match &self.location {
        // Updating a side area is a runtime no-op.
        Ok(location) => location.workspace_name.clone()?,
        Err(reason) => {
          return Some(format!(
            "the subject workspace for a config update cannot be predicted \
             because {reason}"
          ));
        }
      },
    };

    let renamed = match &mut self.workspace_state {
      Ok(workspace_state) => {
        match workspace_state
          .update_workspace_config(&target_name, new_config)
        {
          Ok(renamed) => renamed,
          Err(reason) => {
            return Some(format!(
              "the workspace config update cannot be predicted because \
               {reason}"
            ));
          }
        }
      }
      Err(reason) => {
        return Some(format!(
          "the workspace config update cannot be predicted because {reason}"
        ));
      }
    };

    if renamed {
      self.invalidate_workspace_state(
        "a preceding workspace rename changes target resolution",
      );
    }
    None
  }

  /// Preflights moving the subject's workspace between monitors.
  fn move_workspace(&mut self, direction: &Direction) {
    self.location = self.location.clone().and_then(|mut current| {
      if current.workspace_name.is_none() {
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
        self.workspace_state = Err(
          "a preceding workspace move changes active and displayed \
           workspace state unpredictably"
            .to_string(),
        );
      }
      Ok(current)
    });
  }

  /// Marks workspace target resolution as unsafe after a rename.
  fn invalidate_workspace_state(&mut self, reason: &str) {
    self.workspace_state = Err(reason.to_string());
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
      InvokeCommand::Focus(args) => preflight.focus(args),
      InvokeCommand::UpdateWorkspaceConfig {
        workspace,
        new_config,
      } => {
        if let Some(reason) = preflight
          .update_workspace_config(workspace.as_deref(), new_config)
        {
          return Some(reason);
        }
      }
      InvokeCommand::FocusNextTab
      | InvokeCommand::FocusPreviousTab
      | InvokeCommand::WmCycleFocus { .. } => {
        preflight.invalidate_workspace_state(
          "a preceding focus-cycle command changes focus unpredictably",
        );
      }
      InvokeCommand::WmReloadConfig => {
        preflight.location = Err(
          "a preceding config reload can rebuild monitor side areas"
            .to_string(),
        );
        preflight.invalidate_workspace_state(
          "a preceding config reload changes workspace state",
        );
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
  state: &mut PredictedWorkspaceState,
) -> Result<PredictedWindowLocation, String> {
  let origin_name = current.workspace_move_origin(state)?;
  let Some(target_name) =
    state.resolve_workspace_target(&origin_name, target)?
  else {
    return Ok(current);
  };
  let target_monitor = state.ensure_workspace_active(&target_name)?;
  if current.workspace_name.as_deref() != Some(&target_name) {
    state.can_focus_workspace = false;
  }
  Ok(PredictedWindowLocation::regular_workspace(
    target_name,
    target_monitor,
  ))
}

#[cfg(test)]
mod tests {
  use uuid::Uuid;
  use wm_common::{MatchType, MonitorMatchConfig, ParsedConfig, SideArea};

  use super::*;
  use crate::{
    commands::{
      container::set_focused_descendant, monitor::ensure_side_areas,
    },
    models::{Monitor, TilingWindow, Workspace},
    test_utils::state_with_monitors,
  };

  fn side_area_ids(monitor: &Monitor) -> [Uuid; 2] {
    [
      monitor.side_area(SideArea::Left).unwrap().id(),
      monitor.side_area(SideArea::Right).unwrap().id(),
    ]
  }

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
        device_name: Some(MatchType::Equals {
          equals: "DISPLAY2".to_string(),
        }),
        hardware_id: None,
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

  #[allow(clippy::too_many_lines)]
  fn assert_focus_then_recent_rule_preflight(
    side: SideArea,
    starts_on_selected_monitor: bool,
  ) {
    let subject = TilingWindow::mock()
      .process_name("widget".to_string())
      .call();
    let focus_anchor = TilingWindow::mock().call();
    let selected_workspace = Workspace::mock()
      .name("1".to_string())
      .tiling_containers(if starts_on_selected_monitor {
        vec![subject.clone().into()]
      } else {
        vec![focus_anchor.clone().into()]
      })
      .call();
    let selected_monitor = Monitor::mock()
      .device_name("DISPLAY1".to_string())
      .workspaces(vec![selected_workspace.clone()])
      .call();
    let other_workspace = Workspace::mock()
      .name("2".to_string())
      .tiling_containers(if starts_on_selected_monitor {
        vec![focus_anchor.clone().into()]
      } else {
        vec![subject.clone().into()]
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
    set_focused_descendant(&focus_anchor.clone().into(), None);

    let subject_workspace = if starts_on_selected_monitor {
      &selected_workspace
    } else {
      &other_workspace
    };
    state.recent_workspace_name =
      Some(subject_workspace.config().name.clone());
    let initial_recent = state.recent_workspace_name.clone();
    let initial_focus_id = state.focused_container().unwrap().id();
    let side_name = match side {
      SideArea::Left => "left",
      SideArea::Right => "right",
    };
    let parsed_config = serde_yaml::from_str::<ParsedConfig>(&format!(
      r"
general:
  toggle_workspace_on_refocus: false
side_areas:
  left: 300px
  right: 300px
  match:
    - device_name: {{ equals: DISPLAY1 }}
window_rules:
  - commands:
      - 'focus --workspace {}'
      - 'move --recent-workspace'
      - 'move --side-area {side_name}'
    match:
      - window_process: {{ equals: widget }}
workspaces:
  - name: '1'
  - name: '2'
",
      subject_workspace.config().name
    ))
    .unwrap();
    let mut config = UserConfig::mock_with_value(parsed_config);

    ensure_side_areas(&selected_monitor, &state, &config).unwrap();
    ensure_side_areas(&other_monitor, &state, &config).unwrap();
    let initial_area_ids = side_area_ids(&selected_monitor);

    let result = run_window_rules(
      subject.clone().into(),
      &WindowRuleEvent::Manage,
      &mut state,
      &mut config,
    );
    assert!(
      result.is_ok(),
      "rule partially applied before side-area failure: {:?}",
      result.err()
    );

    if starts_on_selected_monitor {
      assert_eq!(
        subject.workspace().map(|workspace| workspace.id()),
        Some(selected_workspace.id())
      );
      assert_eq!(
        state.focused_container().unwrap().id(),
        initial_focus_id
      );
      assert_eq!(state.recent_workspace_name, initial_recent);
      assert_eq!(subject.done_window_rules().len(), 0);
    } else {
      assert_eq!(
        subject
          .workspace()
          .and_then(|workspace| workspace.side_area()),
        Some(side)
      );
      assert_eq!(subject.done_window_rules().len(), 1);
    }

    assert_eq!(side_area_ids(&selected_monitor), initial_area_ids);
    assert!(other_monitor.side_area(side).is_none());
  }

  #[allow(clippy::too_many_lines)]
  fn assert_inactive_focus_changes_next_active_preflight(
    side: SideArea,
    activated_on_selected_monitor: bool,
  ) {
    let subject = TilingWindow::mock()
      .process_name("widget".to_string())
      .call();
    let workspace_2_anchor = TilingWindow::mock().call();
    let focus_anchor = TilingWindow::mock().call();
    let workspace_1 = Workspace::mock()
      .name("1".to_string())
      .tiling_containers(vec![subject.clone().into()])
      .call();
    let workspace_2 = Workspace::mock()
      .name("2".to_string())
      .tiling_containers(vec![workspace_2_anchor.into()])
      .call();
    let workspace_4 = Workspace::mock()
      .name("4".to_string())
      .tiling_containers(vec![focus_anchor.clone().into()])
      .call();

    let (selected_workspaces, other_workspaces) =
      if activated_on_selected_monitor {
        (
          vec![workspace_4.clone()],
          vec![workspace_1.clone(), workspace_2.clone()],
        )
      } else {
        (
          vec![workspace_1.clone(), workspace_2.clone()],
          vec![workspace_4.clone()],
        )
      };
    let selected_monitor = Monitor::mock()
      .device_name("DISPLAY1".to_string())
      .workspaces(selected_workspaces)
      .call();
    let other_monitor = Monitor::mock()
      .device_name("DISPLAY2".to_string())
      .workspaces(other_workspaces)
      .call();
    let mut state = state_with_monitors(vec![
      selected_monitor.clone(),
      other_monitor.clone(),
    ]);
    set_focused_descendant(&focus_anchor.clone().into(), None);
    state.recent_workspace_name = Some("1".to_string());
    let initial_recent = state.recent_workspace_name.clone();
    let initial_focus_id = state.focused_container().unwrap().id();
    let side_name = match side {
      SideArea::Left => "left",
      SideArea::Right => "right",
    };
    let activation_monitor = usize::from(!activated_on_selected_monitor);
    let parsed_config = serde_yaml::from_str::<ParsedConfig>(&format!(
      r"
general:
  toggle_workspace_on_refocus: false
side_areas:
  left: 300px
  right: 300px
  match:
    - device_name: {{ equals: DISPLAY1 }}
window_rules:
  - commands:
      - 'focus --workspace 3'
      - 'move --next-active-workspace'
      - 'move --side-area {side_name}'
    match:
      - window_process: {{ equals: widget }}
workspaces:
  - name: '1'
  - name: '3'
    bind_to_monitor: {activation_monitor}
  - name: '2'
  - name: '4'
"
    ))
    .unwrap();
    let mut config = UserConfig::mock_with_value(parsed_config);

    ensure_side_areas(&selected_monitor, &state, &config).unwrap();
    ensure_side_areas(&other_monitor, &state, &config).unwrap();
    let initial_area_ids = side_area_ids(&selected_monitor);

    let result = run_window_rules(
      subject.clone().into(),
      &WindowRuleEvent::Manage,
      &mut state,
      &mut config,
    );
    assert!(
      result.is_ok(),
      "rule partially applied after activating workspace 3: {:?}",
      result.err()
    );

    if activated_on_selected_monitor {
      assert_eq!(
        subject
          .workspace()
          .and_then(|workspace| workspace.side_area()),
        Some(side)
      );
      assert_eq!(subject.done_window_rules().len(), 1);
      assert_eq!(
        state
          .workspace_by_name("3")
          .and_then(|workspace| workspace.monitor())
          .map(|monitor| monitor.id()),
        Some(selected_monitor.id())
      );
    } else {
      assert_eq!(
        subject.workspace().map(|workspace| workspace.id()),
        Some(workspace_1.id())
      );
      assert_eq!(
        state.focused_container().unwrap().id(),
        initial_focus_id
      );
      assert_eq!(state.recent_workspace_name, initial_recent);
      assert!(state.workspace_by_name("3").is_none());
      assert_eq!(subject.done_window_rules().len(), 0);
    }

    assert_eq!(side_area_ids(&selected_monitor), initial_area_ids);
    assert!(other_monitor.side_area(side).is_none());
  }

  #[allow(clippy::too_many_lines)]
  fn assert_keep_alive_update_changes_next_active_preflight(
    side: SideArea,
    starts_on_selected_monitor: bool,
  ) {
    let subject = TilingWindow::mock()
      .process_name("widget".to_string())
      .call();
    let workspace_3_anchor = TilingWindow::mock().call();
    let workspace_1 = Workspace::mock()
      .name("1".to_string())
      .tiling_containers(vec![subject.clone().into()])
      .call();
    let workspace_2 = Workspace::mock().name("2".to_string()).call();
    let mut workspace_2_config = workspace_2.config();
    workspace_2_config.keep_alive = true;
    workspace_2.set_config(workspace_2_config);
    let workspace_3 = Workspace::mock()
      .name("3".to_string())
      .tiling_containers(vec![workspace_3_anchor.into()])
      .call();

    let (selected_workspaces, other_workspaces) =
      if starts_on_selected_monitor {
        (
          vec![workspace_1.clone(), workspace_2.clone()],
          vec![workspace_3.clone()],
        )
      } else {
        (
          vec![workspace_3.clone()],
          vec![workspace_1.clone(), workspace_2.clone()],
        )
      };
    let selected_monitor = Monitor::mock()
      .device_name("DISPLAY1".to_string())
      .workspaces(selected_workspaces)
      .call();
    let other_monitor = Monitor::mock()
      .device_name("DISPLAY2".to_string())
      .workspaces(other_workspaces)
      .call();
    let mut state = state_with_monitors(vec![
      selected_monitor.clone(),
      other_monitor.clone(),
    ]);
    set_focused_descendant(&subject.clone().into(), None);
    state.recent_workspace_name = Some("3".to_string());

    let initial_focus_id = state.focused_container().unwrap().id();
    let initial_recent = state.recent_workspace_name.clone();
    let initial_workspace_ids = state
      .workspaces()
      .into_iter()
      .map(|workspace| workspace.id())
      .collect::<Vec<_>>();
    let workspace_2_id = workspace_2.id();
    let side_name = match side {
      SideArea::Left => "left",
      SideArea::Right => "right",
    };
    let parsed_config = serde_yaml::from_str::<ParsedConfig>(&format!(
      r"
general:
  toggle_workspace_on_refocus: false
side_areas:
  left: 300px
  right: 300px
  match:
    - device_name: {{ equals: DISPLAY1 }}
window_rules:
  - commands:
      - 'focus --workspace 3'
      - 'update-workspace-config --workspace 2 --keep-alive false'
      - 'focus --workspace 1'
      - 'move --next-active-workspace'
      - 'move --side-area {side_name}'
    match:
      - window_process: {{ equals: widget }}
workspaces:
  - name: '1'
  - name: '2'
    keep_alive: true
  - name: '3'
"
    ))
    .unwrap();
    let mut config = UserConfig::mock_with_value(parsed_config);

    ensure_side_areas(&selected_monitor, &state, &config).unwrap();
    ensure_side_areas(&other_monitor, &state, &config).unwrap();
    let initial_area_ids = side_area_ids(&selected_monitor);

    let result = run_window_rules(
      subject.clone().into(),
      &WindowRuleEvent::Manage,
      &mut state,
      &mut config,
    );
    assert!(
      result.is_ok(),
      "rule partially applied after changing keep_alive: {:?}",
      result.err()
    );

    if starts_on_selected_monitor {
      assert_eq!(
        subject.workspace().map(|workspace| workspace.id()),
        Some(workspace_1.id())
      );
      assert_eq!(
        state.focused_container().unwrap().id(),
        initial_focus_id
      );
      assert_eq!(state.recent_workspace_name, initial_recent);
      assert_eq!(
        state
          .workspaces()
          .into_iter()
          .map(|workspace| workspace.id())
          .collect::<Vec<_>>(),
        initial_workspace_ids
      );
      assert!(workspace_2.config().keep_alive);
      assert_eq!(
        state.workspace_by_name("2").unwrap().id(),
        workspace_2_id
      );
      assert_eq!(subject.done_window_rules().len(), 0);
    } else {
      assert_eq!(
        subject
          .workspace()
          .and_then(|workspace| workspace.side_area()),
        Some(side)
      );
      assert_eq!(subject.done_window_rules().len(), 1);
      assert!(!workspace_2.config().keep_alive);
      assert!(state.workspace_by_name("2").is_none());
      assert!(state.container_by_id(workspace_2_id).is_none());
      assert_eq!(
        state
          .sorted_workspaces(&config)
          .into_iter()
          .map(|workspace| workspace.config().name)
          .collect::<Vec<_>>(),
        vec!["1".to_string(), "3".to_string()]
      );
    }

    assert_eq!(side_area_ids(&selected_monitor), initial_area_ids);
    assert!(other_monitor.side_area(side).is_none());
  }

  fn assert_subject_workspace_config_target(
    side: SideArea,
    update_before_side_area: bool,
  ) {
    let subject = TilingWindow::mock()
      .process_name("widget".to_string())
      .call();
    let workspace = Workspace::mock()
      .name("1".to_string())
      .tiling_containers(vec![subject.clone().into()])
      .call();
    let monitor = Monitor::mock()
      .device_name("DISPLAY1".to_string())
      .workspaces(vec![workspace.clone()])
      .call();
    let mut state = state_with_monitors(vec![monitor.clone()]);
    set_focused_descendant(&subject.clone().into(), None);
    let side_name = match side {
      SideArea::Left => "left",
      SideArea::Right => "right",
    };
    let commands = if update_before_side_area {
      format!(
        "['update-workspace-config --keep-alive true', \
         'move --side-area {side_name}']"
      )
    } else {
      format!(
        "['move --side-area {side_name}', \
         'update-workspace-config --keep-alive true']"
      )
    };
    let parsed_config = serde_yaml::from_str::<ParsedConfig>(&format!(
      r"
side_areas:
  left: 300px
  right: 300px
window_rules:
  - commands: {commands}
    match:
      - window_process: {{ equals: widget }}
workspaces:
  - name: '1'
"
    ))
    .unwrap();
    let mut config = UserConfig::mock_with_value(parsed_config);
    ensure_side_areas(&monitor, &state, &config).unwrap();

    run_window_rules(
      subject.clone().into(),
      &WindowRuleEvent::Manage,
      &mut state,
      &mut config,
    )
    .unwrap();

    assert_eq!(
      subject
        .workspace()
        .and_then(|subject_workspace| subject_workspace.side_area()),
      Some(side)
    );
    assert_eq!(workspace.config().keep_alive, update_before_side_area);
    assert_eq!(subject.done_window_rules().len(), 1);
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

  #[test]
  fn manage_preflights_focus_then_recent_workspace_for_both_sides() {
    for side in [SideArea::Left, SideArea::Right] {
      assert_focus_then_recent_rule_preflight(side, true);
      assert_focus_then_recent_rule_preflight(side, false);
    }
  }

  #[test]
  fn manage_preflights_inactive_focus_then_next_active_for_both_sides() {
    for side in [SideArea::Left, SideArea::Right] {
      assert_inactive_focus_changes_next_active_preflight(side, false);
      assert_inactive_focus_changes_next_active_preflight(side, true);
    }
  }

  #[test]
  fn manage_preflights_keep_alive_updates_before_both_side_areas() {
    for side in [SideArea::Left, SideArea::Right] {
      assert_keep_alive_update_changes_next_active_preflight(side, true);
      assert_keep_alive_update_changes_next_active_preflight(side, false);
    }
  }

  #[test]
  fn manage_resolves_subject_workspace_config_targets() {
    for side in [SideArea::Left, SideArea::Right] {
      assert_subject_workspace_config_target(side, true);
      assert_subject_workspace_config_target(side, false);
    }
  }
}
