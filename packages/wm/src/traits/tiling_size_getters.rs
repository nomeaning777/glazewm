use std::cell::Ref;

use ambassador::delegatable_trait;
use anyhow::Context;
use wm_common::{GapsConfig, TilingDirection};

use super::{CommonGetters, TilingDirectionGetters};
use crate::models::{Container, DirectionContainer, TilingContainer};

pub const MIN_TILING_SIZE: f32 = 0.01;

#[delegatable_trait]
pub trait TilingSizeGetters: CommonGetters {
  fn tiling_size(&self) -> f32;

  fn set_tiling_size(&self, tiling_size: f32);

  fn gaps_config(&self) -> Ref<'_, GapsConfig>;

  fn set_gaps_config(&self, gaps_config: GapsConfig);

  /// Gets the horizontal and vertical gaps between windows in pixels.
  fn inner_gaps(&self) -> anyhow::Result<(i32, i32)> {
    let monitor = self.monitor().context("No monitor.")?;
    let monitor_rect = monitor.native_properties().bounds;
    let gaps_config = self.gaps_config();

    let scale_factor = if gaps_config.scale_with_dpi {
      monitor.native_properties().scale_factor
    } else {
      1.
    };

    Ok((
      gaps_config
        .inner_gap
        .to_px(monitor_rect.height(), Some(scale_factor)),
      gaps_config
        .inner_gap
        .to_px(monitor_rect.width(), Some(scale_factor)),
    ))
  }

  /// Gets the container to resize when resizing a tiling window.
  fn container_to_resize(
    &self,
    is_width_resize: bool,
  ) -> anyhow::Result<Option<TilingContainer>> {
    if let Some(tabbed_parent) =
      self.parent().and_then(|parent| parent.as_tabbed().cloned())
    {
      return tabbed_parent.container_to_resize(is_width_resize);
    }

    let parent = self.direction_container().context("No parent.")?;

    let tiling_direction = parent.tiling_direction();

    // Whether the resize is in the inverse of its tiling direction.
    let is_inverse_resize = match tiling_direction {
      TilingDirection::Horizontal => !is_width_resize,
      TilingDirection::Vertical => is_width_resize,
    };

    let container_to_resize = if is_inverse_resize {
      match parent {
        // Prevent workspaces from being resized.
        DirectionContainer::Split(parent) => Some(parent.into()),
        DirectionContainer::Workspace(_) => None,
      }
    } else if self.tiling_siblings().count() > 0 {
      // Window can only be resized if it has siblings.
      Some(self.as_tiling_container()?)
    } else {
      let grandparent = parent.parent().context("No grandparent.")?;

      // Resize grandparent in layouts like H[1 V[2 H[3]]], where
      // container 3 is resized horizontally.
      match grandparent {
        Container::Split(grandparent) => Some(grandparent.into()),
        Container::Tabbed(grandparent) => {
          return grandparent.container_to_resize(is_width_resize);
        }
        _ => None,
      }
    };

    if let Some(tabbed_parent) = container_to_resize
      .as_ref()
      .and_then(CommonGetters::parent)
      .and_then(|parent| parent.as_tabbed().cloned())
    {
      return tabbed_parent.container_to_resize(is_width_resize);
    }

    Ok(container_to_resize)
  }
}

/// Implements the `TilingSizeGetters` trait for a given struct.
///
/// Expects that the struct has a wrapping `RefCell` containing a struct
/// with a `tiling_size` field.
#[macro_export]
macro_rules! impl_tiling_size_getters {
  ($struct_name:ident) => {
    impl TilingSizeGetters for $struct_name {
      fn tiling_size(&self) -> f32 {
        self.0.borrow().tiling_size
      }

      fn set_tiling_size(&self, tiling_size: f32) {
        self.0.borrow_mut().tiling_size = tiling_size;
      }

      fn gaps_config(&self) -> Ref<'_, GapsConfig> {
        Ref::map(self.0.borrow(), |inner| &inner.gaps_config)
      }

      fn set_gaps_config(&self, gaps_config: GapsConfig) {
        self.0.borrow_mut().gaps_config = gaps_config;
      }
    }
  };
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::models::{
    SplitContainer, TabbedContainer, TilingWindow, Workspace,
  };

  #[test]
  fn resize_inside_split_tab_targets_outer_tab_slot() {
    for split_direction in
      [TilingDirection::Horizontal, TilingDirection::Vertical]
    {
      let window = TilingWindow::mock().call();
      let split = SplitContainer::mock()
        .tiling_direction(split_direction)
        .tiling_containers(vec![window.clone().into()])
        .call();
      let tabbed = TabbedContainer::mock()
        .tiling_containers(vec![split.into()])
        .call();
      let sibling = TilingWindow::mock().call();
      let _workspace = Workspace::mock()
        .tiling_direction(TilingDirection::Horizontal)
        .tiling_containers(vec![tabbed.clone().into(), sibling.into()])
        .call();

      let target = window.container_to_resize(true).unwrap().unwrap();
      assert_eq!(target.id(), tabbed.id());
    }
  }
}
