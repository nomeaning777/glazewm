use std::collections::VecDeque;

use anyhow::{bail, Context};

use crate::{
  models::{Container, TabbedContainer, TilingContainer},
  traits::{CommonGetters, TilingSizeGetters},
};

/// Wraps tiling children from the same parent in a new tabbed container.
pub fn wrap_in_tabbed_container(
  tabbed_container: &TabbedContainer,
  target_parent: &Container,
  target_children: &[TilingContainer],
) -> anyhow::Result<()> {
  if target_children.is_empty() {
    bail!("Cannot create an empty tabbed container.");
  }

  if !tabbed_container.is_detached() || tabbed_container.has_children() {
    bail!("Tabbed container must be empty and detached.");
  }

  if target_children
    .iter()
    .any(|child| child.parent().as_ref() != Some(target_parent))
  {
    bail!("All tab children must have the same target parent.");
  }

  let target_children_ids = target_children
    .iter()
    .map(CommonGetters::id)
    .collect::<Vec<_>>();
  let ordered_target_children = target_parent
    .tiling_children()
    .filter(|child| target_children_ids.contains(&child.id()))
    .collect::<Vec<_>>();
  if ordered_target_children.len() != target_children.len() {
    bail!("Tab children contain duplicates or invalid children.");
  }

  let child_indices = ordered_target_children
    .iter()
    .map(CommonGetters::index)
    .collect::<Vec<_>>();

  let starting_index = child_indices
    .into_iter()
    .min()
    .context("Failed to get starting child index.")?;

  let sorted_focus_ids = target_parent
    .borrow_child_focus_order()
    .iter()
    .filter(|id| target_children_ids.contains(id))
    .copied()
    .collect::<VecDeque<_>>();

  let starting_focus_index = target_parent
    .borrow_child_focus_order()
    .iter()
    .position(|id| target_children_ids.contains(id))
    .context("Failed to get starting focus index.")?;

  let total_tiling_size = target_children
    .iter()
    .map(TilingSizeGetters::tiling_size)
    .sum::<f32>();

  if total_tiling_size <= 0.0 {
    bail!("Tab children must have a positive total tiling size.");
  }

  target_parent
    .borrow_children_mut()
    .insert(starting_index, tabbed_container.clone().into());
  target_parent
    .borrow_child_focus_order_mut()
    .insert(starting_focus_index, tabbed_container.id());

  *tabbed_container.borrow_parent_mut() = Some(target_parent.clone());
  tabbed_container.set_tiling_size(total_tiling_size);

  for target_child in &ordered_target_children {
    *target_child.borrow_parent_mut() =
      Some(tabbed_container.clone().into());
    target_child
      .set_tiling_size(target_child.tiling_size() / total_tiling_size);

    tabbed_container
      .borrow_children_mut()
      .push_back(target_child.clone().into());

    target_parent
      .borrow_children_mut()
      .retain(|child| child.id() != target_child.id());
    target_parent
      .borrow_child_focus_order_mut()
      .retain(|id| id != &target_child.id());
  }

  *tabbed_container.borrow_child_focus_order_mut() = sorted_focus_ids;

  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::GapsConfig;

  use super::*;
  use crate::{
    models::{TilingWindow, Workspace},
    traits::CommonGetters,
  };

  #[test]
  fn preserves_child_and_focus_order() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();

    workspace.borrow_child_focus_order_mut().swap(0, 1);

    let tabbed = TabbedContainer::new(GapsConfig::default());
    wrap_in_tabbed_container(
      &tabbed,
      &workspace.clone().into(),
      &[first.clone().into(), second.clone().into()],
    )
    .unwrap();

    assert_eq!(workspace.child_count(), 1);
    assert_eq!(workspace.children()[0].id(), tabbed.id());
    assert_eq!(
      tabbed
        .children()
        .iter()
        .map(CommonGetters::id)
        .collect::<Vec<_>>(),
      vec![first.id(), second.id()]
    );
    assert_eq!(
      tabbed
        .child_focus_order()
        .map(|child| child.id())
        .collect::<Vec<_>>(),
      vec![second.id(), first.id()]
    );
    assert!((first.tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((second.tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn rejects_children_from_different_parents() {
    let first = TilingWindow::mock().call();
    let last = TilingWindow::mock().call();
    let first_workspace = Workspace::mock()
      .tiling_containers(vec![first.clone().into()])
      .call();
    let second_workspace = Workspace::mock()
      .tiling_containers(vec![last.clone().into()])
      .call();

    let result = wrap_in_tabbed_container(
      &TabbedContainer::new(GapsConfig::default()),
      &first_workspace.into(),
      &[first.into(), last.into()],
    );

    assert!(result.is_err());
    assert_eq!(second_workspace.child_count(), 1);
  }

  #[test]
  fn adding_a_tab_keeps_normalized_layout_weights() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let third = TilingWindow::mock().call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();
    let tabbed = TabbedContainer::new(GapsConfig::default());

    wrap_in_tabbed_container(
      &tabbed,
      &workspace.into(),
      &[first.clone().into(), second.clone().into()],
    )
    .unwrap();
    super::super::attach_container(
      &third.clone().into(),
      &tabbed.clone().into(),
      None,
    )
    .unwrap();

    for child in [first, second, third] {
      assert!((child.tiling_size() - (1.0 / 3.0)).abs() < f32::EPSILON);
    }
  }
}
