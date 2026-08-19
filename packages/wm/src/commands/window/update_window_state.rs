use anyhow::Context;
use tracing::{info, warn};
use wm_common::WindowState;

use crate::{
  commands::container::{
    attach_container, move_container_within_tree, replace_container,
    resize_tiling_container,
  },
  models::{Container, InsertionTarget, TilingContainer, WindowContainer},
  traits::{CommonGetters, TilingSizeGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

/// Updates the state of a window.
///
/// Adds the window for redraw if there is a state change.
///
/// Returns the window after the state change.
pub fn update_window_state(
  window: WindowContainer,
  target_state: WindowState,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<WindowContainer> {
  if window.state() == target_state {
    return Ok(window);
  }

  info!("Updating window state: {:?}.", target_state);

  match target_state {
    WindowState::Tiling => set_tiling(&window, state, config),
    _ => set_non_tiling(window, target_state, state),
  }
}

/// Updates the state of a window to be `WindowState::Tiling`.
fn set_tiling(
  window: &WindowContainer,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<WindowContainer> {
  let window = window
    .as_non_tiling_window()
    .context("Invalid window state.")?
    .clone();

  let workspace =
    window.workspace().context("Window has no workspace.")?;

  // Check whether the previous insertion target is still valid. Restore
  // layout containers that were automatically removed because taking the
  // window out left them empty (for example, a singleton tabbed layout).
  let insertion_target = match window.insertion_target() {
    Some(insertion_target)
      if restore_insertion_target_parent(&insertion_target, state)? =>
    {
      Some(insertion_target)
    }
    _ => None,
  };

  // Get the position in the tree to insert the new tiling window. This
  // will be the window's previous tiling position if it has one, or
  // instead beside the last focused tiling window in the workspace.
  let (target_parent, target_index) = insertion_target
    .as_ref()
    .map(|insertion_target| {
      (
        insertion_target.target_parent.clone(),
        insertion_target.target_index,
      )
    })
    // Fallback to the last focused tiling window within the workspace.
    .or_else(|| {
      let focused_window = workspace
        .descendant_focus_order()
        .find(Container::is_tiling_window)?;

      let parent = focused_window.parent()?;
      Some((parent, focused_window.index() + 1))
    })
    // Default to inserting at the end of the workspace.
    .unwrap_or((workspace.clone().into(), workspace.child_count()));

  let tiling_window = window.to_tiling(config.value.gaps.clone());

  // Replace the original window with the created tiling window.
  replace_container(
    &tiling_window.clone().into(),
    &window.parent().context("No parent.")?,
    window.index(),
  )?;

  move_container_within_tree(
    &tiling_window.clone().into(),
    &target_parent,
    target_index,
    state,
  )?;

  #[allow(clippy::cast_precision_loss)]
  if let Some(insertion_target) = &insertion_target {
    restore_tiling_size(&tiling_window.clone().into(), insertion_target);
  }

  state
    .pending_sync
    .queue_containers_to_redraw(target_parent.tiling_children())
    .queue_workspace_to_reorder(workspace);

  Ok(tiling_window.into())
}

/// Updates the state of a window to be either `WindowState::Floating`,
/// `WindowState::Fullscreen`, or `WindowState::Minimized`.
fn set_non_tiling(
  window: WindowContainer,
  target_state: WindowState,
  state: &mut WmState,
) -> anyhow::Result<WindowContainer> {
  // A window can only be updated to a minimized state if it is
  // natively minimized.
  // TODO: Consider doing the same for maximized and fullscreen states.
  if target_state == WindowState::Minimized
    && !window.native_properties().is_minimized
  {
    info!("No window state update. Minimizing window.");

    // TODO: Instead of doing the platform call directly here, instead add
    // a `queue_state_change` method to `PendingSync`.
    if let Err(err) = window.native().minimize() {
      warn!("Failed to minimize window: {}", err);
    }

    return Ok(window);
  }

  let workspace = window.workspace().context("No workspace.")?;

  match window {
    WindowContainer::NonTilingWindow(window) => {
      let current_state = window.state();

      // Update the window's previous state if the discriminant changes.
      // TODO: Move out handling of active drag. Can then simplify calls to
      // `set_active_drag` in `handle_window_moved_or_resized_end`.
      if !current_state.is_same_state(&target_state)
        && window.active_drag().is_none()
      {
        window.set_prev_state(current_state);
        state.pending_sync.queue_workspace_to_reorder(workspace);
      }

      window.set_state(target_state);
      state.pending_sync.queue_container_to_redraw(window.clone());

      Ok(window.into())
    }
    WindowContainer::TilingWindow(window) => {
      let parent = window.parent().context("No parent")?;
      let insertion_target =
        capture_insertion_target(&window.clone().into())?;

      let non_tiling_window =
        window.to_non_tiling(target_state.clone(), Some(insertion_target));

      // Non-tiling windows should always be direct children of the
      // workspace.
      if parent != workspace.clone().into() {
        move_container_within_tree(
          &window.clone().into(),
          &workspace.clone().into(),
          workspace.child_count(),
          state,
        )?;
      }

      replace_container(
        &non_tiling_window.clone().into(),
        &workspace.clone().into(),
        window.index(),
      )?;

      state
        .pending_sync
        .queue_container_to_redraw(non_tiling_window.clone())
        .queue_containers_to_redraw(workspace.tiling_children())
        .queue_workspace_to_reorder(workspace);

      Ok(non_tiling_window.into())
    }
  }
}

/// Captures a tiling container's current insertion position.
///
/// If its parent has no other children, also capture the parent's position
/// recursively. Those layout containers will be removed as they become
/// empty and can then be recreated when the window returns to tiling.
fn capture_insertion_target(
  container: &TilingContainer,
) -> anyhow::Result<InsertionTarget> {
  let target_parent = container.parent().context("No parent.")?;
  let target_parent_restore = if target_parent.child_count() == 1 {
    target_parent
      .as_tiling_container()
      .ok()
      .map(|parent| capture_insertion_target(&parent).map(Box::new))
      .transpose()?
  } else {
    None
  };

  Ok(InsertionTarget {
    target_parent,
    target_index: container.index(),
    prev_tiling_size: container.tiling_size(),
    prev_sibling_count: container.tiling_siblings().count(),
    target_parent_restore,
  })
}

/// Ensures the insertion target's parent is attached to a displayed
/// workspace, restoring layout ancestors that were automatically removed.
fn restore_insertion_target_parent(
  insertion_target: &InsertionTarget,
  state: &mut WmState,
) -> anyhow::Result<bool> {
  if insertion_target
    .target_parent
    .workspace()
    .is_some_and(|workspace| workspace.is_displayed())
  {
    return Ok(true);
  }

  let Some(parent_insertion_target) =
    &insertion_target.target_parent_restore
  else {
    return Ok(false);
  };

  // Only recreate layout containers which are still detached and empty.
  // A non-empty or attached target indicates that the layout changed for
  // another reason after the window left it.
  if !insertion_target.target_parent.is_detached()
    || insertion_target.target_parent.has_children()
    || !restore_insertion_target_parent(parent_insertion_target, state)?
  {
    return Ok(false);
  }

  attach_container(
    &insertion_target.target_parent,
    &parent_insertion_target.target_parent,
    Some(parent_insertion_target.target_index),
  )?;

  if let Ok(tiling_parent) =
    insertion_target.target_parent.as_tiling_container()
  {
    restore_tiling_size(&tiling_parent, parent_insertion_target);
  }

  state.pending_sync.queue_container_to_redraw(
    parent_insertion_target.target_parent.clone(),
  );

  Ok(true)
}

/// Restores a container's previous logical size, scaling it if the number
/// of siblings changed while it was away.
#[allow(clippy::cast_precision_loss)]
fn restore_tiling_size(
  container: &TilingContainer,
  insertion_target: &InsertionTarget,
) {
  let siblings = container.tiling_siblings().collect::<Vec<_>>();
  if siblings.is_empty() {
    container.set_tiling_size(1.0);
    return;
  }

  let size_scale = (insertion_target.prev_sibling_count + 1) as f32
    / (siblings.len() + 1) as f32;

  // E.g. if the container was 0.5 with one sibling and now has two, use
  // 0.5 * (2/3) to maintain proportional sizing.
  let target_size = insertion_target.prev_tiling_size * size_scale;

  if container.parent().is_some_and(|parent| parent.is_tabbed()) {
    // Logical tab weights may be smaller than the split-layout minimum;
    // tabs do not divide the visible rectangle while the layout is active.
    let target_size = target_size.clamp(f32::EPSILON, 1.0 - f32::EPSILON);
    let sibling_size = siblings
      .iter()
      .map(TilingSizeGetters::tiling_size)
      .sum::<f32>();

    if sibling_size > 0.0 {
      for sibling in &siblings {
        sibling.set_tiling_size(
          sibling.tiling_size() / sibling_size * (1.0 - target_size),
        );
      }
    } else {
      let sibling_size = (1.0 - target_size) / siblings.len() as f32;
      for sibling in &siblings {
        sibling.set_tiling_size(sibling_size);
      }
    }

    container.set_tiling_size(target_size);
  } else {
    resize_tiling_container(container, target_size);
  }
}

#[cfg(test)]
mod tests {
  use tokio::sync::mpsc;
  use wm_common::{FloatingStateConfig, GapsConfig};
  use wm_platform::Dispatcher;

  use super::*;
  use crate::{
    commands::container::{
      attach_container, detach_container, wrap_in_tabbed_container,
    },
    models::{
      Monitor, NonTilingWindow, SplitContainer, TabbedContainer,
      TilingWindow, Workspace,
    },
  };

  fn state_with_workspace(workspace: Workspace) -> (WmState, Monitor) {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (exit_tx, _exit_rx) = mpsc::unbounded_channel();
    let state = WmState::new(Dispatcher::mock(), event_tx, exit_tx);
    let monitor = Monitor::mock().workspaces(vec![workspace]).call();

    attach_container(
      &monitor.clone().into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();

    (state, monitor)
  }

  #[test]
  fn returning_to_tiling_restores_tab_and_logical_size() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();
    let tabbed = TabbedContainer::new(GapsConfig::default());
    wrap_in_tabbed_container(
      &tabbed,
      &workspace.clone().into(),
      &[first.clone().into(), second.clone().into()],
    )
    .unwrap();
    first.set_tiling_size(0.25);
    second.set_tiling_size(0.75);
    let (mut state, monitor) = state_with_workspace(workspace);
    let config = UserConfig::mock();

    let floating = update_window_state(
      first.clone().into(),
      WindowState::Floating(FloatingStateConfig::default()),
      &mut state,
      &config,
    )
    .unwrap();
    assert_eq!(tabbed.child_count(), 1);
    assert_eq!(tabbed.children()[0].id(), second.id());

    let restored = update_window_state(
      floating,
      WindowState::Tiling,
      &mut state,
      &config,
    )
    .unwrap();

    assert_eq!(
      tabbed
        .children()
        .iter()
        .map(CommonGetters::id)
        .collect::<Vec<_>>(),
      vec![first.id(), second.id()]
    );
    assert_eq!(restored.id(), first.id());
    assert!(
      (restored.as_tiling_container().unwrap().tiling_size() - 0.25).abs()
        < f32::EPSILON
    );
    assert!((second.tiling_size() - 0.75).abs() < f32::EPSILON);

    detach_container(monitor.into()).unwrap();
  }

  #[test]
  fn new_tiling_tab_is_inserted_after_focused_tab() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let tabbed = TabbedContainer::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();
    let floating = NonTilingWindow::mock().call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![tabbed.clone().into()])
      .non_tiling_windows(vec![floating.clone()])
      .call();
    let (mut state, monitor) = state_with_workspace(workspace);

    let restored = update_window_state(
      floating.into(),
      WindowState::Tiling,
      &mut state,
      &UserConfig::mock(),
    )
    .unwrap();

    assert_eq!(
      tabbed
        .children()
        .iter()
        .map(CommonGetters::id)
        .collect::<Vec<_>>(),
      vec![first.id(), restored.id(), second.id()]
    );

    detach_container(monitor.into()).unwrap();
  }

  #[test]
  fn returning_to_tiling_rebuilds_tab_through_empty_layout_ancestors() {
    let window = TilingWindow::mock().call();
    let tabbed = TabbedContainer::mock()
      .tiling_containers(vec![window.clone().into()])
      .call();
    let split = SplitContainer::mock()
      .tiling_containers(vec![tabbed.clone().into()])
      .call();
    let sibling = TilingWindow::mock().call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![
        split.clone().into(),
        sibling.clone().into(),
      ])
      .call();
    split.set_tiling_size(0.25);
    sibling.set_tiling_size(0.75);
    let (mut state, monitor) = state_with_workspace(workspace.clone());
    let config = UserConfig::mock();

    let floating = update_window_state(
      window.clone().into(),
      WindowState::Floating(FloatingStateConfig::default()),
      &mut state,
      &config,
    )
    .unwrap();
    assert!(tabbed.is_detached());
    assert!(split.is_detached());
    assert!((sibling.tiling_size() - 1.0).abs() < f32::EPSILON);

    update_window_state(
      floating,
      WindowState::Tiling,
      &mut state,
      &config,
    )
    .unwrap();

    // The redundant singleton split is normalized away after insertion,
    // while the explicit tabbed layout is retained.
    assert!(split.is_detached());
    assert_eq!(
      workspace
        .tiling_children()
        .map(|child| child.id())
        .collect::<Vec<_>>(),
      vec![tabbed.id(), sibling.id()]
    );
    assert_eq!(tabbed.tiling_children().next().unwrap().id(), window.id());
    assert!((tabbed.tiling_size() - 0.25).abs() < 1e-5);
    assert!((sibling.tiling_size() - 0.75).abs() < 1e-5);

    detach_container(monitor.into()).unwrap();
  }
}
