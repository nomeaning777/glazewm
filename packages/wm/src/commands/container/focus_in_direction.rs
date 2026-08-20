use anyhow::Context;
use wm_common::{TilingDirection, WindowState};
use wm_platform::Direction;

use super::set_focused_descendant;
use crate::{
  models::{Container, TilingContainer},
  traits::{CommonGetters, TilingDirectionGetters, WindowGetters},
  wm_state::WmState,
};

pub fn focus_in_direction(
  origin_container: &Container,
  direction: &Direction,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let is_in_side_area = origin_container
    .workspace()
    .is_some_and(|workspace| workspace.is_side_area());

  let focus_target = match origin_container {
    Container::TilingWindow(_) => {
      // If a suitable focus target isn't found in the current workspace,
      // attempt to find a workspace in the given direction.
      tiling_focus_target(origin_container, direction)?.map_or_else(
        || {
          if is_in_side_area {
            Ok(None)
          } else {
            workspace_focus_target(origin_container, direction, state)
          }
        },
        |container| Ok(Some(container)),
      )?
    }
    Container::NonTilingWindow(ref non_tiling_window) => {
      match non_tiling_window.state() {
        WindowState::Floating(_) => {
          floating_focus_target(origin_container, direction)
        }
        WindowState::Fullscreen(_) => {
          workspace_focus_target(origin_container, direction, state)?
        }
        _ => None,
      }
    }
    Container::Workspace(_) => {
      workspace_focus_target(origin_container, direction, state)?
    }
    _ => None,
  };

  // i3 treats tabbed containers as horizontally oriented. If no target
  // exists outside the stack, wrap to the opposite tab.
  let focus_target = focus_target
    .or_else(|| tabbed_wrap_target(origin_container, direction));

  // Set focus to the target container.
  if let Some(focus_target) = focus_target {
    set_focused_descendant(&focus_target, None);
    state.pending_sync.queue_focus_change().queue_cursor_jump();
  }

  Ok(())
}

/// Gets an adjacent direct tab when moving horizontally.
fn tabbed_focus_target(
  tab: &Container,
  direction: &Direction,
) -> Option<Container> {
  if !matches!(direction, Direction::Left | Direction::Right) {
    return None;
  }

  if !tab.parent().is_some_and(|parent| parent.is_tabbed()) {
    return None;
  }
  let target = match direction {
    Direction::Left => tab.prev_siblings().next(),
    Direction::Right => tab.next_siblings().next(),
    _ => None,
  }?;

  Some(focused_leaf_or_self(target))
}

/// Wraps focus within the innermost tabbed container.
fn tabbed_wrap_target(
  origin_container: &Container,
  direction: &Direction,
) -> Option<Container> {
  if !matches!(direction, Direction::Left | Direction::Right) {
    return None;
  }

  let tabbed_parent = origin_container
    .ancestors()
    .find_map(|ancestor| ancestor.as_tabbed().cloned())?;
  let children = tabbed_parent.children();

  if children.len() < 2 {
    return None;
  }

  let target = match direction {
    Direction::Left => children.back().cloned(),
    Direction::Right => children.front().cloned(),
    _ => None,
  }?;

  Some(focused_leaf_or_self(target))
}

fn focused_leaf_or_self(container: Container) -> Container {
  let focused_descendant = container.descendant_focus_order().next();
  focused_descendant.unwrap_or(container)
}

fn floating_focus_target(
  origin_container: &Container,
  direction: &Direction,
) -> Option<Container> {
  let is_floating = |sibling: &Container| {
    sibling.as_non_tiling_window().is_some_and(|window| {
      matches!(window.state(), WindowState::Floating(_))
    })
  };

  let mut floating_siblings =
    origin_container.siblings().filter(is_floating);

  // Wrap if next/previous floating window is not found.
  match direction {
    Direction::Left => origin_container
      .next_siblings()
      .find(is_floating)
      .or_else(|| floating_siblings.last()),
    Direction::Right => origin_container
      .prev_siblings()
      .find(is_floating)
      .or_else(|| floating_siblings.next()),
    // Cannot focus vertically from a floating window.
    _ => None,
  }
}

/// Gets a focus target within the current workspace. Traverse upwards from
/// the origin container to find an adjacent container that can be focused.
fn tiling_focus_target(
  origin_container: &Container,
  direction: &Direction,
) -> anyhow::Result<Option<Container>> {
  let tiling_direction = TilingDirection::from_direction(direction);
  let mut origin_or_ancestor = origin_container.clone();

  // Traverse upwards from the focused container. Stop searching when a
  // workspace is encountered.
  while !origin_or_ancestor.is_workspace() {
    let parent_container =
      origin_or_ancestor.parent().context("No parent.")?;

    // Tabbed containers behave as horizontally oriented at their own
    // boundary. Inner split siblings have already been checked because
    // traversal proceeds from the focused leaf outward.
    let Ok(parent) = parent_container.as_direction_container() else {
      if parent_container.is_tabbed() {
        if let Some(target) =
          tabbed_focus_target(&origin_or_ancestor, direction)
        {
          return Ok(Some(target));
        }

        origin_or_ancestor = parent_container;
        continue;
      }

      return Err(anyhow::anyhow!("No direction container."));
    };

    // Skip if the tiling direction doesn't match.
    if parent.tiling_direction() != tiling_direction {
      origin_or_ancestor = parent.into();
      continue;
    }

    // Get the next/prev tiling sibling depending on the tiling direction.
    let focus_target = match direction {
      Direction::Up | Direction::Left => origin_or_ancestor
        .prev_siblings()
        .find_map(|c| c.as_tiling_container().ok()),
      _ => origin_or_ancestor
        .next_siblings()
        .find_map(|c| c.as_tiling_container().ok()),
    };

    match focus_target {
      Some(target) => {
        // Return once a suitable focus target is found.
        return Ok(match target {
          TilingContainer::TilingWindow(_) => Some(target.into()),
          TilingContainer::Split(split) => split
            .descendant_in_direction(&direction.inverse())
            .map(Into::into),
          TilingContainer::Tabbed(tabbed) => {
            tabbed.active_child().map(focused_leaf_or_self)
          }
        });
      }
      None => origin_or_ancestor = parent.into(),
    }
  }

  Ok(None)
}

/// Gets a focus target outside of the current workspace in the given
/// direction.
///
/// This will descend into the workspace in the given direction, and will
/// always return a tiling container. This makes it different from the
/// `focus_workspace` command with `FocusWorkspaceTarget::Direction`.
fn workspace_focus_target(
  origin_container: &Container,
  direction: &Direction,
  state: &WmState,
) -> anyhow::Result<Option<Container>> {
  let monitor = origin_container.monitor().context("No monitor.")?;

  let target_workspace = state
    .monitor_in_direction(&monitor, direction)?
    .and_then(|monitor| monitor.displayed_workspace());

  let focused_fullscreen = target_workspace
    .as_ref()
    .and_then(|workspace| workspace.descendant_focus_order().next())
    .filter(|focused| match focused {
      Container::NonTilingWindow(window) => {
        matches!(window.state(), WindowState::Fullscreen(_))
      }
      _ => false,
    });

  let focus_target = focused_fullscreen
    .or_else(|| {
      target_workspace.as_ref().and_then(|workspace| {
        workspace
          .descendant_in_direction(&direction.inverse())
          .map(Into::into)
      })
    })
    .or(target_workspace.map(Into::into));

  Ok(focus_target)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::models::{TabbedContainer, TilingWindow};

  #[test]
  fn focuses_adjacent_tabs_horizontally() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let _tabbed = TabbedContainer::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();

    assert_eq!(
      tabbed_focus_target(&first.clone().into(), &Direction::Right)
        .map(|target| target.id()),
      Some(second.id())
    );
    assert_eq!(
      tabbed_focus_target(&second.clone().into(), &Direction::Left)
        .map(|target| target.id()),
      Some(first.id())
    );
    assert!(tabbed_focus_target(&first.into(), &Direction::Down).is_none());
  }

  #[test]
  fn searches_outer_tabbed_container_at_inner_edge() {
    let inner_window = TilingWindow::mock().call();
    let inner = TabbedContainer::mock()
      .tiling_containers(vec![inner_window.clone().into()])
      .call();
    let outer_window = TilingWindow::mock().call();
    let _outer = TabbedContainer::mock()
      .tiling_containers(vec![
        inner.clone().into(),
        outer_window.clone().into(),
      ])
      .call();

    assert_eq!(
      tiling_focus_target(&inner_window.into(), &Direction::Right)
        .unwrap()
        .map(|target| target.id()),
      Some(outer_window.id())
    );
  }

  #[test]
  fn prefers_split_sibling_inside_active_tab() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let split = crate::models::SplitContainer::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();
    let other_tab = TilingWindow::mock().call();
    let _tabbed = TabbedContainer::mock()
      .tiling_containers(vec![split.into(), other_tab.into()])
      .call();

    assert_eq!(
      tiling_focus_target(&first.into(), &Direction::Right)
        .unwrap()
        .map(|target| target.id()),
      Some(second.id())
    );
  }

  #[test]
  fn wraps_innermost_tabbed_container() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let _tabbed = TabbedContainer::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();

    assert_eq!(
      tabbed_wrap_target(&first.clone().into(), &Direction::Left)
        .map(|target| target.id()),
      Some(second.id())
    );
    assert_eq!(
      tabbed_wrap_target(&second.into(), &Direction::Right)
        .map(|target| target.id()),
      Some(first.id())
    );
  }
}
