use crate::{
  commands::container::{
    flatten_child_split_containers, move_container_within_tree,
  },
  models::{WindowContainer, Workspace},
  traits::CommonGetters,
  wm_state::WmState,
};

/// Moves every child out of a side area without retaining references that
/// can become stale when the source layout is flattened after each move.
pub(super) fn move_side_area_contents(
  source_area: &Workspace,
  target_workspace: &Workspace,
  state: &WmState,
) -> anyhow::Result<Vec<WindowContainer>> {
  let mut moved_windows = Vec::new();

  while let Some(child) = source_area.children().front().cloned() {
    moved_windows.extend(
      child
        .self_and_descendants()
        .filter_map(|container| container.as_window_container().ok()),
    );

    move_container_within_tree(
      &child,
      &target_workspace.clone().into(),
      target_workspace.child_count(),
      state,
    )?;
  }

  flatten_child_split_containers(&target_workspace.clone().into())?;

  Ok(moved_windows)
}
