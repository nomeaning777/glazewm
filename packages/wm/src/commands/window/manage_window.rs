use anyhow::Context;
use tracing::info;
use wm_common::{try_warn, WindowRuleEvent, WindowState, WmEvent};
use wm_platform::{NativeWindow, RectDelta};

use crate::{
  commands::{
    container::{attach_container, set_focused_descendant},
    window::run_window_rules,
  },
  models::{
    Container, Monitor, NativeWindowProperties, NonTilingWindow,
    TilingWindow, WindowContainer,
  },
  traits::{CommonGetters, PositionGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn manage_window(
  native_window: NativeWindow,
  target_parent: Option<Container>,
  state: &mut WmState,
  config: &mut UserConfig,
) -> anyhow::Result<()> {
  let Some(native_properties) =
    check_is_manageable(&native_window).unwrap_or(None)
  else {
    return Ok(());
  };

  // Create the window instance. This may fail if the window handle has
  // already been destroyed.
  let window = try_warn!(create_window(
    native_window,
    native_properties,
    target_parent,
    state,
    config
  ));

  // Set the newly added window as focus descendant. This means the window
  // rules will be run as if the window is focused.
  set_focused_descendant(&window.clone().into(), None);

  // Window might be detached if `ignore` command has been invoked.
  let updated_window = run_window_rules(
    window.clone(),
    &WindowRuleEvent::Manage,
    state,
    config,
  )?;

  if let Some(window) = updated_window {
    info!("New window managed: {window}");

    state.emit_event(WmEvent::WindowManaged {
      managed_window: window.to_dto()?,
    });

    // OS focus should be set to the newly added window in case it's not
    // already focused.
    state.pending_sync.queue_focus_change();

    // Window rules can move a new window into a persistent side area.
    // Redraw the main workspace too because its usable bounds depend on
    // the side area widths.
    if window
      .workspace()
      .is_some_and(|workspace| workspace.is_side_area())
    {
      if let Some(displayed_workspace) = window
        .monitor()
        .and_then(|monitor| monitor.displayed_workspace())
      {
        state
          .pending_sync
          .queue_container_to_redraw(displayed_workspace);
      }
    }

    // Normally, a `PlatformEvent::WindowFocused` event is what triggers
    // focus effects and workspace reordering to be applied. However, when
    // a window is first launched, this event can come before the
    // window is managed, and so we need to force an update here.
    state.pending_sync.queue_focused_effect_update();
    state.pending_sync.queue_workspace_to_reorder(
      window.workspace().context("No workspace.")?,
    );

    // Sibling containers need to be redrawn if the window is tiling.
    state.pending_sync.queue_container_to_redraw(
      if window.state() == WindowState::Tiling {
        window.parent().context("No parent.")?
      } else {
        window.into()
      },
    );
  }

  Ok(())
}

/// Checks if a window is manageable and retrieves its native properties.
///
/// Returns `Ok(Some(properties))` if the window is manageable and its
/// properties were retrieved successfully.
fn check_is_manageable(
  native_window: &NativeWindow,
) -> anyhow::Result<Option<NativeWindowProperties>> {
  if !native_window.is_visible()? {
    return Ok(None);
  }

  #[cfg(target_os = "macos")]
  {
    use wm_platform::NativeWindowExtMacOs;

    let is_standard_window = native_window.role()? == "AXWindow"
      && native_window.subrole()? == "AXStandardWindow";

    if !is_standard_window {
      return Ok(None);
    }
  }

  // Ensure window has a valid process name, title, etc.
  let native_properties = NativeWindowProperties::try_from(native_window)?;

  #[cfg(target_os = "windows")]
  {
    use wm_platform::{
      NativeWindowWindowsExt, WS_CAPTION, WS_CHILD, WS_EX_NOACTIVATE,
      WS_EX_TOOLWINDOW,
    };

    // TODO: Temporary fix for managing Flow Launcher until a force manage
    // command is added.
    let is_flow_launcher = native_properties.process_name
      == "Flow.Launcher"
      && native_properties.title == "Flow.Launcher";

    if !is_flow_launcher {
      // Ensure window is top-level (i.e. not a child window). Ignore
      // windows that cannot be focused or if they're unavailable in
      // task switcher (alt+tab menu).
      if native_window.has_window_style(WS_CHILD)
        || native_window
          .has_window_style_ex(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW)
      {
        return Ok(None);
      }

      // Some applications spawn top-level windows for menus that
      // should be ignored. This includes the autocomplete popup in
      // Notepad++ and title bar menu in Keepass. Although not
      // foolproof, these can typically be identified by having an
      // owner window and no title bar.
      if native_window.has_owner_window()
        && !native_window.has_window_style(WS_CAPTION)
      {
        return Ok(None);
      }
    }
  }

  Ok(Some(native_properties))
}

fn create_window(
  native_window: NativeWindow,
  native_properties: NativeWindowProperties,
  target_parent: Option<Container>,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<WindowContainer> {
  let nearest_monitor = state
    .nearest_monitor(&native_window)
    .context("No nearest monitor.")?;

  let nearest_workspace = nearest_monitor
    .displayed_workspace()
    .context("No nearest workspace.")?;

  let gaps_config = config.value.gaps.clone();
  let window_state =
    window_state_to_create(&native_properties, &nearest_monitor, config)?;

  let (target_parent, target_index) =
    insertion_target(&window_state, target_parent, state)?;

  let target_workspace =
    target_parent.workspace().context("No target workspace.")?;

  let prefers_centered = config
    .value
    .window_behavior
    .state_defaults
    .floating
    .centered;

  // Calculate where window should be placed when floating is enabled. Use
  // the original width/height of the window and optionally position it in
  // the center of the workspace.
  let is_same_workspace = nearest_workspace.id() == target_workspace.id();
  let floating_placement = {
    let placement = if !is_same_workspace || prefers_centered {
      native_properties
        .frame
        .translate_to_center(&target_workspace.to_rect()?)
    } else {
      native_properties.frame.clone()
    };

    // Clamp the window size to be within the workspace's outer gaps. 10px
    // is arbitrary - helps differentiate from tiling windows.
    let max_workspace_rect = target_workspace.max_workspace_rect()?;
    placement.clamp_size(
      max_workspace_rect.width() - 10,
      max_workspace_rect.height() - 10,
    )
  };

  // Window has no border delta unless it's later changed via the
  // `adjust_borders` command.
  let border_delta = RectDelta::zero();

  let window_container: WindowContainer = match window_state {
    WindowState::Tiling => TilingWindow::new(
      None,
      native_window,
      native_properties,
      None,
      border_delta,
      floating_placement,
      false,
      gaps_config,
      Vec::new(),
      None,
    )
    .into(),
    _ => NonTilingWindow::new(
      None,
      native_window,
      native_properties,
      window_state,
      None,
      border_delta,
      None,
      floating_placement,
      !prefers_centered,
      Vec::new(),
      None,
    )
    .into(),
  };

  attach_container(
    &window_container.clone().into(),
    &target_parent,
    Some(target_index),
  )?;

  // The OS might spawn the window on a different monitor to the target
  // parent, so adjustments might need to be made because of DPI.
  if nearest_monitor
    .has_dpi_difference(&window_container.clone().into())?
  {
    window_container.set_has_pending_dpi_adjustment(true);
  }

  Ok(window_container)
}

/// Gets the initial state for a window based on its native state.
///
/// Note that maximized windows are initialized as tiling.
fn window_state_to_create(
  native_properties: &NativeWindowProperties,
  nearest_monitor: &Monitor,
  config: &UserConfig,
) -> anyhow::Result<WindowState> {
  if native_properties.is_minimized {
    return Ok(WindowState::Minimized);
  }

  let nearest_workspace = nearest_monitor
    .displayed_workspace()
    .context("No workspace.")?;

  // Only initialize as fullscreen if the window *exceeds* the workspace
  // bounds (due to the 1px inset).
  //
  // For example, with 0px outer gaps and a window that covers the entire
  // workspace, it would still not be initialized as fullscreen. The window
  // needs to be within the workspace's outer gaps by at least 1px on each
  // side.
  if !native_properties.is_maximized
    && native_properties
      .frame
      .inset(1)
      .contains_rect(&nearest_workspace.max_workspace_rect()?)
  {
    return Ok(WindowState::Fullscreen(
      config
        .value
        .window_behavior
        .state_defaults
        .fullscreen
        .clone(),
    ));
  }

  // Initialize windows that can't be resized as floating.
  if !native_properties.is_resizable {
    return Ok(WindowState::Floating(
      config.value.window_behavior.state_defaults.floating.clone(),
    ));
  }

  Ok(WindowState::default_from_config(&config.value))
}

/// Gets where to insert a new window in the container tree.
///
/// Rules:
/// - Side areas redirect to their monitor's displayed main workspace.
/// - Otherwise, a supplied target parent receives the window at index 0.
/// - For non-tiling windows: Always append to the workspace.
/// - For tiling windows:
///   1. Try to insert after the focused tiling window if one exists.
///   2. If a non-tiling window is focused, try to insert after the first
///      tiling window found.
///   3. If no tiling windows exist, append to the workspace.
///
/// Returns tuple of (parent container, insertion index).
fn insertion_target(
  window_state: &WindowState,
  target_parent: Option<Container>,
  state: &WmState,
) -> anyhow::Result<(Container, usize)> {
  let has_target_parent = target_parent.is_some();
  let focused_container = target_parent
    .or_else(|| state.focused_container())
    .context("No focused container.")?;

  let focused_workspace =
    focused_container.workspace().context("No workspace.")?;

  // Only an explicit move (including a window rule) may enter a side
  // area. Use the main workspace's focus history for automatic insertion.
  let (focused_container, focused_workspace) =
    if focused_workspace.is_side_area() {
      let workspace = focused_workspace
        .monitor()
        .and_then(|monitor| monitor.displayed_workspace())
        .context("No displayed main workspace on side area's monitor.")?;
      (workspace.clone().into(), workspace)
    } else if has_target_parent {
      return Ok((focused_container, 0));
    } else {
      (focused_container, focused_workspace)
    };

  // For tiling windows, try to find a suitable tiling window to insert
  // next to.
  if *window_state == WindowState::Tiling {
    let sibling = match focused_container {
      Container::TilingWindow(_) => Some(focused_container),
      _ => focused_workspace
        .descendant_focus_order()
        .find(Container::is_tiling_window),
    };

    if let Some(sibling) = sibling {
      if let Some(tabbed_parent) = sibling
        .parent()
        .and_then(|parent| parent.as_tabbed().cloned())
      {
        return Ok((tabbed_parent.clone().into(), sibling.index() + 1));
      }

      return Ok((
        sibling.parent().context("No parent.")?,
        sibling.index() + 1,
      ));
    }
  }

  // Default to appending to workspace.
  Ok((
    focused_workspace.clone().into(),
    focused_workspace.child_count(),
  ))
}

#[cfg(test)]
mod tests {
  use wm_common::{FloatingStateConfig, ParsedConfig, SideArea};

  use super::*;
  use crate::{
    commands::window::move_window_to_side_area,
    models::{SplitContainer, TabbedContainer, Workspace},
    test_utils::{assert_tree_links_and_focus_order, state_with_monitors},
  };

  fn new_window_states() -> [WindowState; 2] {
    [
      WindowState::Tiling,
      WindowState::Floating(FloatingStateConfig::default()),
    ]
  }

  fn insert_mock_window(
    window_state: &WindowState,
    target_parent: Option<Container>,
    state: &WmState,
  ) -> WindowContainer {
    let (parent, index) =
      insertion_target(window_state, target_parent, state).unwrap();
    let window: WindowContainer = if *window_state == WindowState::Tiling {
      TilingWindow::mock()
        .process_name("new-app".into())
        .call()
        .into()
    } else {
      NonTilingWindow::mock()
        .state(window_state.clone())
        .process_name("new-app".into())
        .call()
        .into()
    };
    attach_container(&window.clone().into(), &parent, Some(index))
      .unwrap();
    set_focused_descendant(&window.clone().into(), None);
    assert_tree_links_and_focus_order(
      &state.root_container.clone().into(),
    );
    assert_eq!(
      state.focused_container().map(|focused| focused.id()),
      Some(window.id())
    );
    window
  }

  fn workspace_with_layout(
    layout: &str,
  ) -> (Workspace, Container, TilingWindow, NonTilingWindow) {
    let first = TilingWindow::mock().call();
    let focused = TilingWindow::mock().call();
    let last = TilingWindow::mock().call();
    let floating = NonTilingWindow::mock().call();
    let children = vec![first.into(), focused.clone().into(), last.into()];
    let workspace = Workspace::mock()
      .tiling_containers(match layout {
        "split" => vec![SplitContainer::mock()
          .tiling_containers(children)
          .call()
          .into()],
        "tabbed" => vec![TabbedContainer::mock()
          .tiling_containers(children)
          .call()
          .into()],
        _ => children,
      })
      .non_tiling_windows(vec![floating.clone()])
      .call();
    let parent = focused.parent().unwrap();
    (workspace, parent, focused, floating)
  }

  fn side_area_with_focus(
    side: SideArea,
    layout: &str,
  ) -> (Workspace, Container) {
    let window = TilingWindow::mock().call();
    let floating = NonTilingWindow::mock().call();
    let area = Workspace::mock_side_area()
      .side(side)
      .tiling_containers(match layout {
        "split" => vec![SplitContainer::mock()
          .tiling_containers(vec![window.clone().into()])
          .call()
          .into()],
        "tabbed" => vec![TabbedContainer::mock()
          .tiling_containers(vec![window.clone().into()])
          .call()
          .into()],
        "empty" => vec![],
        _ => vec![window.clone().into()],
      })
      .non_tiling_windows(if layout == "floating" {
        vec![floating.clone()]
      } else {
        vec![]
      })
      .call();
    let focused = match layout {
      "floating" => floating.into(),
      "empty" => area.clone().into(),
      _ => window.into(),
    };
    (area, focused)
  }

  #[test]
  fn sidebar_focus_inserts_into_displayed_main_workspace() {
    for side in [SideArea::Left, SideArea::Right] {
      for side_layout in ["tiling", "split", "tabbed", "floating", "empty"]
      {
        for main_layout in ["tiling", "split", "tabbed"] {
          for window_state in new_window_states() {
            let (main, parent, last_focused, floating) =
              workspace_with_layout(main_layout);
            let (area, side_focus) =
              side_area_with_focus(side, side_layout);
            let hidden = Workspace::mock().name("hidden".into()).call();
            let monitor = Monitor::mock()
              .workspaces(vec![hidden.clone(), main.clone(), area.clone()])
              .call();
            let other_monitor = Monitor::mock()
              .workspaces(vec![Workspace::mock()
                .name("other".into())
                .call()])
              .call();
            let state =
              state_with_monitors(vec![other_monitor, monitor.clone()]);
            set_focused_descendant(&last_focused.into(), None);
            set_focused_descendant(&floating.into(), None);
            set_focused_descendant(&side_focus, None);
            let side_children = area.children();
            let main_child_count = main.child_count();

            let window = insert_mock_window(&window_state, None, &state);

            assert_eq!(
              window.workspace().unwrap().id(),
              main.id(),
              "{side:?}, {side_layout}, {main_layout}, {window_state:?}"
            );
            if window_state == WindowState::Tiling {
              assert_eq!(window.parent().unwrap().id(), parent.id());
              assert_eq!(window.index(), 2);
            } else {
              assert_eq!(window.parent().unwrap().id(), main.id());
              assert_eq!(window.index(), main_child_count);
            }
            assert_eq!(area.children(), side_children);
            assert!(!hidden.has_children());
            assert_eq!(
              monitor.displayed_workspace().unwrap().id(),
              main.id()
            );
          }
        }
      }
    }
  }

  #[test]
  fn normal_workspace_focus_preserves_insertion_rules() {
    for layout in ["tiling", "split", "tabbed"] {
      for focus_floating in [false, true] {
        for window_state in new_window_states() {
          let (workspace, parent, focused, floating) =
            workspace_with_layout(layout);
          let monitor =
            Monitor::mock().workspaces(vec![workspace.clone()]).call();
          let state = state_with_monitors(vec![monitor]);
          set_focused_descendant(&focused.into(), None);
          if focus_floating {
            set_focused_descendant(&floating.into(), None);
          }
          let child_count = workspace.child_count();

          let window = insert_mock_window(&window_state, None, &state);

          if window_state == WindowState::Tiling {
            assert_eq!(window.parent().unwrap().id(), parent.id());
            assert_eq!(window.index(), 2);
          } else {
            assert_eq!(window.parent().unwrap().id(), workspace.id());
            assert_eq!(window.index(), child_count);
          }
        }
      }
    }
  }

  #[test]
  fn sidebar_focus_with_empty_main_workspace_appends_to_main() {
    for side in [SideArea::Left, SideArea::Right] {
      for window_state in new_window_states() {
        let main = Workspace::mock().call();
        let (area, focused) = side_area_with_focus(side, "tabbed");
        let monitor =
          Monitor::mock().workspaces(vec![main.clone(), area]).call();
        let state = state_with_monitors(vec![monitor]);
        set_focused_descendant(&focused, None);

        let window = insert_mock_window(&window_state, None, &state);

        assert_eq!(window.parent().unwrap().id(), main.id());
        assert_eq!(window.index(), 0);
      }
    }
  }

  #[test]
  fn side_area_target_parent_uses_its_own_monitors_main_workspace() {
    for side in [SideArea::Left, SideArea::Right] {
      for layout in ["tiling", "split", "tabbed", "floating", "empty"] {
        for window_state in new_window_states() {
          let (main, parent, focused, _) = workspace_with_layout("tabbed");
          let (area, side_focus) = side_area_with_focus(side, layout);
          let monitor = Monitor::mock()
            .workspaces(vec![main.clone(), area.clone()])
            .call();
          let other_workspace =
            Workspace::mock().name("other".into()).call();
          let other_monitor = Monitor::mock()
            .workspaces(vec![other_workspace.clone()])
            .call();
          let state =
            state_with_monitors(vec![other_monitor, monitor.clone()]);
          set_focused_descendant(&focused.into(), None);
          set_focused_descendant(&other_workspace.clone().into(), None);
          let target_parent = if layout == "split" || layout == "tabbed" {
            side_focus.parent().unwrap()
          } else {
            area.clone().into()
          };
          let side_children = area.children();

          let window =
            insert_mock_window(&window_state, Some(target_parent), &state);

          assert_eq!(window.workspace().unwrap().id(), main.id());
          if window_state == WindowState::Tiling {
            assert_eq!(window.parent().unwrap().id(), parent.id());
            assert_eq!(window.index(), 2);
          } else {
            assert_eq!(window.parent().unwrap().id(), main.id());
          }
          assert_eq!(area.children(), side_children);
          assert!(!other_workspace.has_children());
          assert_eq!(
            monitor.displayed_workspace().unwrap().id(),
            main.id()
          );
        }
      }
    }
  }

  #[test]
  fn regular_target_parent_preserves_prepend_behavior() {
    for layout in ["tiling", "split", "tabbed"] {
      let (workspace, parent, _, _) = workspace_with_layout(layout);
      let (area, focused) = side_area_with_focus(SideArea::Left, "tiling");
      let monitor =
        Monitor::mock().workspaces(vec![workspace, area]).call();
      let state = state_with_monitors(vec![monitor]);
      set_focused_descendant(&focused, None);

      let window = insert_mock_window(
        &WindowState::Tiling,
        Some(parent.clone()),
        &state,
      );

      assert_eq!(window.parent().unwrap().id(), parent.id());
      assert_eq!(window.index(), 0);
    }
  }

  #[test]
  fn missing_main_workspace_cannot_fall_back_to_side_area() {
    let (area, focused) = side_area_with_focus(SideArea::Left, "tiling");
    let monitor = Monitor::mock().workspaces(vec![area.clone()]).call();
    let state = state_with_monitors(vec![monitor]);
    set_focused_descendant(&focused, None);

    for target_parent in [None, Some(area.into())] {
      assert!(insertion_target(
        &WindowState::Tiling,
        target_parent,
        &state
      )
      .is_err());
    }
  }

  #[test]
  fn explicit_moves_and_manage_rules_can_still_enter_side_areas() {
    for side in [SideArea::Left, SideArea::Right] {
      for use_rule in [false, true] {
        for window_state in new_window_states() {
          let main = Workspace::mock().call();
          let (area, focused) = side_area_with_focus(side, "tiling");
          let monitor = Monitor::mock()
            .workspaces(vec![main.clone(), area.clone()])
            .call();
          let mut state = state_with_monitors(vec![monitor.clone()]);
          set_focused_descendant(&focused, None);
          let window = insert_mock_window(&window_state, None, &state);
          assert_eq!(window.workspace().unwrap().id(), main.id());

          if use_rule {
            let side_name = if side == SideArea::Left {
              "left"
            } else {
              "right"
            };
            let parsed = serde_yaml::from_str::<ParsedConfig>(&format!(
              "window_rules:\n  - commands: ['move --side-area {side_name}']\n    match:\n      - window_process: {{ equals: new-app }}\nworkspaces:\n  - name: '1'\n"
            )).unwrap();
            let mut config = UserConfig::mock_with_value(parsed);
            let updated = run_window_rules(
              window.clone(),
              &WindowRuleEvent::Manage,
              &mut state,
              &mut config,
            )
            .unwrap()
            .unwrap();
            assert_eq!(updated.id(), window.id());
          } else {
            move_window_to_side_area(
              window.clone(),
              side,
              &mut state,
              &UserConfig::mock(),
            )
            .unwrap();
          }

          assert_eq!(window.workspace().unwrap().id(), area.id());
          assert_eq!(window.state(), window_state);
          assert_eq!(
            monitor.displayed_workspace().unwrap().id(),
            main.id()
          );
          assert_eq!(state.focused_container().unwrap().id(), window.id());
          assert_tree_links_and_focus_order(
            &state.root_container.clone().into(),
          );
        }
      }
    }
  }
}
