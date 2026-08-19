use anyhow::Context;

use super::{
  flatten_tabbed_container, set_focused_descendant,
  wrap_in_tabbed_container,
};
use crate::{
  models::{Container, TabbedContainer, TilingWindow},
  traits::CommonGetters,
  user_config::UserConfig,
  wm_state::WmState,
};

/// Toggles the focused window's parent between split and tabbed layouts.
///
/// Entering tabbed mode groups all top-level tiling siblings on a
/// workspace. Inside a nested split, it changes only the focused
/// container's layout slot. Leaving it flattens the tabbed container back
/// into its parent.
pub fn toggle_tabbed(
  container: &Container,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let Some(window) = focused_tiling_window(container) else {
    return Ok(());
  };

  if let Some(tabbed_parent) =
    nearest_tabbed_parent(&window.clone().into())
  {
    let redraw_parent = tabbed_parent
      .parent()
      .context("Tabbed container has no parent.")?;

    flatten_tabbed_container(tabbed_parent)?;
    set_focused_descendant(&window.clone().into(), None);

    state
      .pending_sync
      .queue_container_to_redraw(redraw_parent)
      .queue_focus_change()
      .queue_cursor_jump();

    return Ok(());
  }

  let parent = window.parent().context("Window has no parent.")?;
  let target_children = parent.tiling_children().collect::<Vec<_>>();

  if target_children.is_empty() {
    return Ok(());
  }

  let tabbed_container = TabbedContainer::new(config.value.gaps.clone());
  wrap_in_tabbed_container(&tabbed_container, &parent, &target_children)?;
  set_focused_descendant(&window.into(), None);

  state
    .pending_sync
    .queue_container_to_redraw(tabbed_container)
    .queue_focus_change()
    .queue_cursor_jump();

  Ok(())
}

/// Focuses the next or previous tab, wrapping at either end.
pub fn focus_tab(
  container: &Container,
  is_next: bool,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let Some(tabbed_parent) = nearest_tabbed_parent(container) else {
    return Ok(());
  };

  let children = tabbed_parent.children();
  if children.len() < 2 {
    return Ok(());
  }

  let active_child = tabbed_parent
    .active_child()
    .context("Tabbed container has no active child.")?;
  let active_index = active_child.index();
  let target_index = if is_next {
    (active_index + 1) % children.len()
  } else {
    (active_index + children.len() - 1) % children.len()
  };

  let target = children
    .get(target_index)
    .cloned()
    .context("Tab focus target does not exist.")?;
  let focused_descendant = target.descendant_focus_order().next();
  let target = focused_descendant.unwrap_or(target);

  set_focused_descendant(&target, None);
  state
    .pending_sync
    .queue_container_to_redraw(tabbed_parent)
    .queue_focus_change()
    .queue_cursor_jump();

  Ok(())
}

fn focused_tiling_window(container: &Container) -> Option<TilingWindow> {
  container.as_tiling_window().cloned().or_else(|| {
    container
      .descendant_focus_order()
      .find_map(|descendant| descendant.as_tiling_window().cloned())
  })
}

fn nearest_tabbed_parent(
  container: &Container,
) -> Option<TabbedContainer> {
  container
    .self_and_ancestors()
    .find_map(|ancestor| ancestor.as_tabbed().cloned())
}

#[cfg(test)]
mod tests {
  use tokio::sync::mpsc;
  use wm_platform::Dispatcher;

  use super::*;
  use crate::{
    models::{SplitContainer, Workspace},
    traits::TilingSizeGetters,
    user_config::UserConfig,
  };

  fn wm_state() -> WmState {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let (exit_tx, _exit_rx) = mpsc::unbounded_channel();
    WmState::new(Dispatcher::mock(), event_tx, exit_tx)
  }

  #[test]
  fn toggles_siblings_into_and_out_of_tabbed_layout() {
    let first = TilingWindow::mock().tiling_size(0.25).call();
    let second = TilingWindow::mock().tiling_size(0.75).call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();
    first.set_tiling_size(0.25);
    second.set_tiling_size(0.75);
    let mut state = wm_state();
    let config = UserConfig::mock();

    toggle_tabbed(&first.clone().into(), &mut state, &config).unwrap();

    let tabbed = workspace.children()[0].as_tabbed().unwrap().clone();
    assert_eq!(workspace.child_count(), 1);
    assert_eq!(
      tabbed
        .children()
        .iter()
        .map(CommonGetters::id)
        .collect::<Vec<_>>(),
      vec![first.id(), second.id()]
    );
    assert_eq!(
      tabbed.active_child().map(|child| child.id()),
      Some(first.id())
    );
    assert!(state.pending_sync.needs_focus_update());

    state.pending_sync.clear();
    toggle_tabbed(&first.clone().into(), &mut state, &config).unwrap();

    assert!(tabbed.is_detached());
    assert_eq!(
      workspace
        .tiling_children()
        .map(|child| child.id())
        .collect::<Vec<_>>(),
      vec![first.id(), second.id()]
    );
    assert!((first.tiling_size() - 0.25).abs() < f32::EPSILON);
    assert!((second.tiling_size() - 0.75).abs() < f32::EPSILON);
    assert!(state.pending_sync.needs_focus_update());
  }

  #[test]
  fn explicit_tab_focus_wraps_in_both_directions() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let third = TilingWindow::mock().call();
    let tabbed = TabbedContainer::mock()
      .tiling_containers(vec![
        first.clone().into(),
        second.into(),
        third.clone().into(),
      ])
      .call();
    let mut state = wm_state();

    focus_tab(&first.clone().into(), false, &mut state).unwrap();
    assert_eq!(
      tabbed.active_child().map(|child| child.id()),
      Some(third.id())
    );

    focus_tab(&third.into(), true, &mut state).unwrap();
    assert_eq!(
      tabbed.active_child().map(|child| child.id()),
      Some(first.id())
    );
    assert!(state.pending_sync.needs_focus_update());
  }

  #[test]
  fn toggling_from_nested_tab_content_flattens_nearest_tabbed_layout() {
    let first = TilingWindow::mock().call();
    let split = SplitContainer::mock()
      .tiling_containers(vec![first.clone().into()])
      .call();
    let second = TilingWindow::mock().call();
    let tabbed = TabbedContainer::mock()
      .tiling_containers(vec![split.clone().into(), second.clone().into()])
      .call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![tabbed.clone().into()])
      .call();
    let mut state = wm_state();

    toggle_tabbed(&first.into(), &mut state, &UserConfig::mock()).unwrap();

    assert!(tabbed.is_detached());
    assert_eq!(
      workspace
        .tiling_children()
        .map(|child| child.id())
        .collect::<Vec<_>>(),
      vec![split.id(), second.id()]
    );
  }
}
