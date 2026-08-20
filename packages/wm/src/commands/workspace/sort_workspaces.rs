use anyhow::Context;
use wm_common::VecDequeExt;

use crate::{
  models::Monitor, traits::CommonGetters, user_config::UserConfig,
};

/// Sorts a monitor's workspaces by config order.
pub fn sort_workspaces(
  monitor: &Monitor,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let mut workspaces = monitor.workspaces();
  config.sort_workspaces(&mut workspaces);
  let offset =
    usize::from(monitor.side_area(wm_common::SideArea::Left).is_some());

  for workspace in &workspaces {
    let target_index = &workspaces
      .iter()
      .position(|sorted_workspace| sorted_workspace.id() == workspace.id())
      .context("Failed to get workspace target index.")?;

    monitor
      .borrow_children_mut()
      .shift_to_index(offset + *target_index, workspace.clone().into());
  }

  Ok(())
}
