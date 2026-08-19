use anyhow::Context;

use super::flatten_empty_tabbed_container;
use crate::{
  models::Container,
  traits::{CommonGetters, TilingSizeGetters, MIN_TILING_SIZE},
};

/// Removes a container from the tree.
///
/// If the container is a tiling container, the siblings will be resized to
/// fill the freed up space. Will flatten empty parent split containers.
#[allow(clippy::needless_pass_by_value)]
pub fn detach_container(child_to_remove: Container) -> anyhow::Result<()> {
  let parent = child_to_remove.parent().context("No parent.")?;

  parent
    .borrow_children_mut()
    .retain(|c| c.id() != child_to_remove.id());

  parent
    .borrow_child_focus_order_mut()
    .retain(|id| *id != child_to_remove.id());

  *child_to_remove.borrow_parent_mut() = None;

  // Resize the siblings if it is a tiling container.
  if let Ok(child_to_remove) = child_to_remove.as_tiling_container() {
    if parent.is_tabbed() {
      // Tab children share a rectangle, but their tiling sizes are kept as
      // logical weights so switching back to a split layout can restore
      // the previous proportions. Normalize the remaining weights
      // after a tab is removed.
      let tiling_siblings = parent.tiling_children().collect::<Vec<_>>();
      let remaining_size = tiling_siblings
        .iter()
        .map(TilingSizeGetters::tiling_size)
        .sum::<f32>();

      if remaining_size > 0.0 {
        for sibling in &tiling_siblings {
          sibling.set_tiling_size(sibling.tiling_size() / remaining_size);
        }
      } else if !tiling_siblings.is_empty() {
        #[allow(clippy::cast_precision_loss)]
        let sibling_size = 1.0 / tiling_siblings.len() as f32;
        for sibling in &tiling_siblings {
          sibling.set_tiling_size(sibling_size);
        }
      }
    } else {
      let tiling_siblings = parent.tiling_children().collect::<Vec<_>>();

      // TODO: Share logic with `resize_tiling_container`.
      let available_size =
        tiling_siblings.iter().fold(0.0, |sum, container| {
          sum + container.tiling_size() - MIN_TILING_SIZE
        });

      if available_size > 0.0 {
        // Adjust size of the siblings based on the freed up space.
        for sibling in &tiling_siblings {
          let resize_factor =
            (sibling.tiling_size() - MIN_TILING_SIZE) / available_size;

          let size_delta = resize_factor * child_to_remove.tiling_size();
          sibling.set_tiling_size(sibling.tiling_size() + size_delta);
        }
      } else if !tiling_siblings.is_empty() {
        let remaining_size = tiling_siblings
          .iter()
          .map(TilingSizeGetters::tiling_size)
          .sum::<f32>();

        if remaining_size > 0.0 {
          for sibling in &tiling_siblings {
            sibling
              .set_tiling_size(sibling.tiling_size() / remaining_size);
          }
        } else {
          #[allow(clippy::cast_precision_loss)]
          let sibling_size = 1.0 / tiling_siblings.len() as f32;
          for sibling in &tiling_siblings {
            sibling.set_tiling_size(sibling_size);
          }
        }
      }
    }
  }

  let attached_parent =
    parent.parent().is_some() || parent.as_workspace().is_some();

  if attached_parent {
    if let Some(split_parent) = parent.as_split().cloned() {
      if split_parent.child_count() == 0 {
        // Detach the empty split through the regular path so its logical
        // size is returned to siblings in the enclosing layout.
        detach_container(split_parent.into())?;
      }
    }

    if let Some(tabbed_parent) = parent.as_tabbed().cloned() {
      flatten_empty_tabbed_container(tabbed_parent)?;
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use wm_common::GapsConfig;

  use super::*;
  use crate::{
    commands::container::wrap_in_tabbed_container,
    models::{SplitContainer, TabbedContainer, TilingWindow, Workspace},
  };

  #[test]
  fn removing_empty_split_returns_its_size_to_outer_siblings() {
    let nested = TilingWindow::mock().call();
    let split = SplitContainer::mock()
      .tiling_containers(vec![nested.clone().into()])
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

    detach_container(nested.into()).unwrap();

    assert!(split.is_detached());
    assert_eq!(workspace.child_count(), 1);
    assert_eq!(workspace.children()[0].id(), sibling.id());
    assert!((sibling.tiling_size() - 1.0).abs() < f32::EPSILON);
  }

  /// A tabbed layout remains active with one child, so new windows
  /// continue to open as tabs and the tab bar remains visible.
  #[test]
  fn preserves_tabbed_container_with_one_remaining_child() {
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
    assert_eq!(workspace.child_count(), 1);

    detach_container(first.clone().into()).unwrap();

    assert!(!tabbed.is_detached());
    assert!(first.is_detached());
    assert_eq!(workspace.child_count(), 1);
    assert_eq!(workspace.children()[0].id(), tabbed.id());
    assert_eq!(tabbed.children()[0].id(), second.id());
    assert!((second.tiling_size() - 1.0).abs() < f32::EPSILON);
  }

  /// Detaching from a stack with three tabs keeps the stack intact.
  #[test]
  fn keeps_tabbed_container_with_multiple_remaining_children() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let third = TilingWindow::mock().call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![
        first.clone().into(),
        second.clone().into(),
        third.clone().into(),
      ])
      .call();
    let tabbed = TabbedContainer::new(GapsConfig::default());

    wrap_in_tabbed_container(
      &tabbed,
      &workspace.clone().into(),
      &[
        first.clone().into(),
        second.clone().into(),
        third.clone().into(),
      ],
    )
    .unwrap();

    detach_container(first.clone().into()).unwrap();

    assert!(!tabbed.is_detached());
    assert_eq!(workspace.children()[0].id(), tabbed.id());
    assert_eq!(
      tabbed
        .children()
        .iter()
        .map(CommonGetters::id)
        .collect::<Vec<_>>(),
      vec![second.id(), third.id()]
    );
  }
}
