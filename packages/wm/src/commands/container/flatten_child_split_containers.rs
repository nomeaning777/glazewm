use super::flatten_split_container;
use crate::{
  models::Container,
  traits::{CommonGetters, TilingDirectionGetters},
};

/// Flattens redundant child layout containers under a direction
/// container.
///
/// For example:
/// ```ignore,compile_fail
/// H[1 H[V[2, 3]]] -> H[1, 2, 3]
/// H[1 H[2, 3]] -> H[1, 2, 3]
/// H[V[1]] -> V[1]
/// ```
pub fn flatten_child_split_containers(
  parent: &Container,
) -> anyhow::Result<()> {
  let Ok(parent) = parent.as_direction_container() else {
    return Ok(());
  };

  // Tabbed containers take one slot in a split, so include every tiling
  // child when deciding whether the parent has a sole layout child.
  let tiling_children = parent
    .children()
    .into_iter()
    .filter(|child| child.as_tiling_container().is_ok())
    .collect::<Vec<_>>();

  if tiling_children.len() == 1 {
    match &tiling_children[0] {
      Container::Split(split_child) => {
        flatten_split_container(split_child.clone())?;
        parent.set_tiling_direction(parent.tiling_direction().inverse());
      }
      Container::Tabbed(tabbed_child) if !tabbed_child.has_children() => {
        super::flatten_empty_tabbed_container(tabbed_child.clone())?;
      }
      _ => {}
    }

    return Ok(());
  }

  let split_children = tiling_children
    .iter()
    .filter_map(|child| child.as_split().cloned())
    .collect::<Vec<_>>();

  for split_child in split_children.iter().filter(|split_child| {
    split_child.tiling_direction() == parent.tiling_direction()
  }) {
    // Additionally flatten redundant top-level split containers in the
    // child.
    if split_child.child_count() == 1 {
      if let Some(split_grandchild) = split_child.children()[0].as_split()
      {
        flatten_split_container(split_grandchild.clone())?;
      }
    }

    flatten_split_container(split_child.clone())?;
  }

  for tabbed_child in tiling_children
    .into_iter()
    .filter_map(|child| child.as_tabbed().cloned())
    .filter(|tabbed| !tabbed.has_children())
    .collect::<Vec<_>>()
  {
    super::flatten_empty_tabbed_container(tabbed_child)?;
  }

  Ok(())
}
