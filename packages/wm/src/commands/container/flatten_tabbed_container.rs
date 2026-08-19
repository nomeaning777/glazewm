use std::collections::VecDeque;

use anyhow::Context;

use crate::{
  models::TabbedContainer,
  traits::{CommonGetters, TilingSizeGetters},
};

/// Removes a tabbed container and moves its children into its parent.
#[allow(clippy::needless_pass_by_value)]
pub fn flatten_tabbed_container(
  tabbed_container: TabbedContainer,
) -> anyhow::Result<()> {
  if !tabbed_container.has_children() {
    return super::detach_container(tabbed_container.into());
  }

  let parent = tabbed_container.parent().context("No parent.")?;
  let index = tabbed_container.index();
  let focus_index = tabbed_container.focus_index();

  let children = tabbed_container.children();
  let child_count = children.len();
  let tabbed_size = tabbed_container.tiling_size();
  let total_child_size = children
    .iter()
    .filter_map(|child| child.as_tiling_container().ok())
    .map(|child| child.tiling_size())
    .sum::<f32>();

  for (child_index, child) in children.iter().cloned().enumerate() {
    *child.borrow_parent_mut() = Some(parent.clone());

    if let Ok(tiling_child) = child.as_tiling_container() {
      #[allow(clippy::cast_precision_loss)]
      let relative_size = if total_child_size > 0.0 {
        tiling_child.tiling_size() / total_child_size
      } else {
        1.0 / child_count.max(1) as f32
      };
      tiling_child.set_tiling_size(tabbed_size * relative_size);
    }

    parent
      .borrow_children_mut()
      .insert(index + child_index, child);
  }

  let child_ids =
    children.iter().map(CommonGetters::id).collect::<Vec<_>>();
  let child_focus_order = tabbed_container
    .borrow_child_focus_order()
    .iter()
    .filter(|id| child_ids.contains(id))
    .copied()
    .collect::<Vec<_>>();
  for (child_focus_index, child_id) in
    child_focus_order.into_iter().enumerate()
  {
    parent
      .borrow_child_focus_order_mut()
      .insert(focus_index + child_focus_index, child_id);
  }

  parent
    .borrow_children_mut()
    .retain(|child| child.id() != tabbed_container.id());
  parent
    .borrow_child_focus_order_mut()
    .retain(|id| *id != tabbed_container.id());

  *tabbed_container.borrow_parent_mut() = None;
  *tabbed_container.borrow_children_mut() = VecDeque::new();
  *tabbed_container.borrow_child_focus_order_mut() = VecDeque::new();

  Ok(())
}

/// Removes a tabbed container if it has no children.
///
/// A singleton tabbed container is intentionally retained. Like i3, tabbed
/// layout remains active with one child and keeps accepting newly opened
/// tabs.
pub fn flatten_empty_tabbed_container(
  tabbed_container: TabbedContainer,
) -> anyhow::Result<()> {
  if tabbed_container.has_children() {
    return Ok(());
  }

  super::detach_container(tabbed_container.into())
}

#[cfg(test)]
mod tests {
  use wm_common::GapsConfig;

  use super::*;
  use crate::{
    commands::container::wrap_in_tabbed_container,
    models::{TabbedContainer, TilingWindow, Workspace},
    traits::CommonGetters,
  };

  #[test]
  fn restores_children_to_parent() {
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
    flatten_tabbed_container(tabbed.clone()).unwrap();

    assert!(tabbed.is_detached());
    assert_eq!(
      workspace
        .tiling_children()
        .map(|child| child.id())
        .collect::<Vec<_>>(),
      vec![first.id(), second.id()]
    );
    assert!((first.tiling_size() - 0.5).abs() < f32::EPSILON);
    assert!((second.tiling_size() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn restores_unequal_child_sizes() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();
    first.set_tiling_size(0.25);
    second.set_tiling_size(0.75);
    let tabbed = TabbedContainer::new(GapsConfig::default());

    wrap_in_tabbed_container(
      &tabbed,
      &workspace.clone().into(),
      &[first.clone().into(), second.clone().into()],
    )
    .unwrap();
    flatten_tabbed_container(tabbed).unwrap();

    assert!((first.tiling_size() - 0.25).abs() < f32::EPSILON);
    assert!((second.tiling_size() - 0.75).abs() < f32::EPSILON);
  }

  #[test]
  fn empty_tabbed_container_is_removed() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();

    let first_size = first.tiling_size();
    let second_size = second.tiling_size();
    let tabbed = TabbedContainer::new(GapsConfig::default());

    wrap_in_tabbed_container(
      &tabbed,
      &workspace.clone().into(),
      &[first.clone().into()],
    )
    .unwrap();
    assert!((tabbed.tiling_size() - first_size).abs() < f32::EPSILON);

    // Singleton tabbed containers are retained.
    flatten_empty_tabbed_container(tabbed.clone()).unwrap();
    assert!(!tabbed.is_detached());

    super::super::detach_container(first.clone().into()).unwrap();

    assert!(tabbed.is_detached());
    assert!(first.is_detached());
    assert_eq!(workspace.children()[0].id(), second.id());
    assert!(
      (second.tiling_size() - (first_size + second_size)).abs()
        < f32::EPSILON
    );
  }
}
