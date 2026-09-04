use tracing::info;
use wm_common::{InvokeCommand, WindowRuleConfig, WindowRuleEvent};

use crate::{
  models::WindowContainer,
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
    if should_defer_side_area_rule(&rule, &subject_window, config) {
      info!(
        "Deferring side-area window rule because the current monitor does \
         not match side_areas.match."
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

/// A side-area rule is not applicable while its window is on a monitor
/// excluded by `side_areas.match`. Defer the whole rule so that its
/// commands are not partially applied and a `run_once` rule remains
/// eligible after a later monitor-selector change.
fn should_defer_side_area_rule(
  rule: &WindowRuleConfig,
  window: &WindowContainer,
  config: &UserConfig,
) -> bool {
  let moves_to_side_area = rule.commands.iter().any(|command| {
    matches!(
      command,
      InvokeCommand::Move(args) if args.side_area.is_some()
    )
  });

  moves_to_side_area
    && window.monitor().is_some_and(|monitor| {
      !config
        .value
        .side_areas
        .matches_monitor(&monitor.native_properties().device_name)
    })
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

  #[test]
  fn manage_side_area_rule_skips_nonmatching_monitors_for_both_sides() {
    assert_manage_rule_respects_monitor_selector(SideArea::Left);
    assert_manage_rule_respects_monitor_selector(SideArea::Right);
  }
}
