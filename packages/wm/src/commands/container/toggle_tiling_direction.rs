use anyhow::Context;
use wm_common::{GapsConfig, TilingDirection, WmEvent};

use super::{flatten_split_container, wrap_in_split_container};
use crate::{
  models::{Container, DirectionContainer, SplitContainer, TilingWindow},
  traits::{CommonGetters, TilingDirectionGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn toggle_tiling_direction(
  container: Container,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let direction_container = match container {
    Container::TilingWindow(tiling_window) => {
      toggle_window_direction(tiling_window, config)
    }
    Container::Workspace(workspace) => {
      workspace
        .set_tiling_direction(workspace.tiling_direction().inverse());

      Ok(workspace.into())
    }
    // Can only toggle tiling direction from a tiling window or workspace.
    _ => return Ok(()),
  }?;

  state.emit_event(WmEvent::TilingDirectionChanged {
    direction_container: direction_container.to_dto()?,
    new_tiling_direction: direction_container.tiling_direction(),
  });

  Ok(())
}

fn toggle_window_direction(
  tiling_window: TilingWindow,
  config: &UserConfig,
) -> anyhow::Result<DirectionContainer> {
  let parent = tiling_window
    .direction_container()
    .context("No direction container.")?;

  // A tabbed parent has a horizontal navigation orientation but is not a
  // split container. Mirroring i3's `split` command, wrap the active tab
  // in a new split instead of changing a direction container outside the
  // stack.
  if tiling_window
    .parent()
    .is_some_and(|parent| parent.is_tabbed())
  {
    return wrap_tab_in_split(
      tiling_window,
      parent.tiling_direction().inverse(),
      &config.value.gaps,
    );
  }

  // If the window is an only child, then either change the tiling
  // direction of its parent workspace or flatten its parent split
  // container.
  if tiling_window.tiling_siblings().count() == 0 {
    return match parent {
      DirectionContainer::Workspace(workspace) => {
        workspace
          .set_tiling_direction(workspace.tiling_direction().inverse());

        Ok(workspace.into())
      }
      DirectionContainer::Split(split_container) => {
        flatten_split_container(split_container.clone())?;

        tiling_window
          .direction_container()
          .context("No direction container.")
      }
    };
  }

  // Create a new split container to wrap the window.
  let split_container = SplitContainer::new(
    parent.tiling_direction().inverse(),
    config.value.gaps.clone(),
  );

  wrap_in_split_container(
    &split_container,
    &parent.into(),
    &[tiling_window.into()],
  )?;

  Ok(split_container.into())
}

fn wrap_tab_in_split(
  tiling_window: TilingWindow,
  tiling_direction: TilingDirection,
  gaps_config: &GapsConfig,
) -> anyhow::Result<DirectionContainer> {
  let tabbed_parent = tiling_window
    .parent()
    .filter(Container::is_tabbed)
    .context("Window does not have a tabbed parent.")?;
  let split_container =
    SplitContainer::new(tiling_direction, gaps_config.clone());

  wrap_in_split_container(
    &split_container,
    &tabbed_parent,
    &[tiling_window.into()],
  )?;

  Ok(split_container.into())
}

pub fn set_tiling_direction(
  container: Container,
  state: &mut WmState,
  config: &UserConfig,
  tiling_direction: &TilingDirection,
) -> anyhow::Result<()> {
  if let Some(tiling_window) = container.as_tiling_window().cloned() {
    if tiling_window
      .parent()
      .is_some_and(|parent| parent.is_tabbed())
    {
      let direction_container = wrap_tab_in_split(
        tiling_window,
        tiling_direction.clone(),
        &config.value.gaps,
      )?;

      state.emit_event(WmEvent::TilingDirectionChanged {
        direction_container: direction_container.to_dto()?,
        new_tiling_direction: direction_container.tiling_direction(),
      });

      return Ok(());
    }
  }

  let direction_container = container
    .direction_container()
    .context("No direction container.")?;

  if direction_container.tiling_direction() == *tiling_direction {
    Ok(())
  } else {
    toggle_tiling_direction(container, state, config)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{
    models::{TabbedContainer, Workspace},
    traits::CommonGetters,
  };

  #[test]
  fn toggling_direction_wraps_only_the_active_tab() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let tabbed = TabbedContainer::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();
    let _workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![tabbed.clone().into()])
      .call();
    let direction_container = wrap_tab_in_split(
      first.clone(),
      TilingDirection::Vertical,
      &GapsConfig::default(),
    )
    .unwrap();

    assert_eq!(
      direction_container.tiling_direction(),
      TilingDirection::Vertical
    );
    assert_eq!(tabbed.child_count(), 2);
    assert_eq!(tabbed.children()[0].id(), direction_container.id());
    assert_eq!(tabbed.children()[1].id(), second.id());
    assert_eq!(direction_container.children()[0].id(), first.id());
  }
}
