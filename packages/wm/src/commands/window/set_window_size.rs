use anyhow::Context;
use wm_common::WindowState;
use wm_platform::{LengthValue, Rect};

use crate::{
  commands::container::resize_tiling_container,
  models::{Container, NonTilingWindow, TilingWindow, WindowContainer},
  traits::{
    CommonGetters, PositionGetters, TilingSizeGetters, WindowGetters,
  },
  wm_state::WmState,
};

/// Arbitrary defaults for minimum floating window dimensions.
const MIN_FLOATING_WIDTH: i32 = 250;
const MIN_FLOATING_HEIGHT: i32 = 140;

pub fn set_window_size(
  window: WindowContainer,
  target_width: Option<LengthValue>,
  target_height: Option<LengthValue>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  match window {
    WindowContainer::TilingWindow(window) => {
      set_tiling_window_size(&window, target_width, target_height, state)?;
    }
    WindowContainer::NonTilingWindow(window) => {
      if matches!(window.state(), WindowState::Floating(_)) {
        set_floating_window_size(
          &window,
          target_width,
          target_height,
          state,
        )?;
      }
    }
  }

  Ok(())
}

fn set_tiling_window_size(
  window: &TilingWindow,
  target_width: Option<LengthValue>,
  target_height: Option<LengthValue>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  if let Some(target_width) = target_width {
    set_tiling_window_length(window, &target_width, true, state)?;
  }

  if let Some(target_height) = target_height {
    set_tiling_window_length(window, &target_height, false, state)?;
  }

  Ok(())
}

/// Updates either the width or height of a tiling window.
fn set_tiling_window_length(
  window: &TilingWindow,
  target_length: &LengthValue,
  is_width_resize: bool,
  state: &mut WmState,
) -> anyhow::Result<()> {
  // When resizing a tiling window, the container to resize can actually be
  // an ancestor split container.
  let container_to_resize = window.container_to_resize(is_width_resize)?;

  if let Some(container_to_resize) = container_to_resize {
    let parent = container_to_resize.parent().context("No parent.")?;
    let (horizontal_gap, vertical_gap) =
      container_to_resize.inner_gaps()?;

    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    let parent_length = if is_width_resize {
      parent.to_rect()?.width()
        - horizontal_gap
          * container_to_resize.tiling_siblings().count() as i32
    } else {
      parent.to_rect()?.height()
        - vertical_gap
          * container_to_resize.tiling_siblings().count() as i32
    };

    // A tab bar occupies layout space outside the native window frame.
    // Add every intervening bar when translating a requested window
    // height into the size of the ancestor container being resized.
    let tab_bar_inset = if is_width_resize {
      0
    } else {
      tab_bar_inset_between(window, &container_to_resize)?
    };
    let target_container_length = target_length
      .to_px(parent_length, None)
      .saturating_add(tab_bar_inset);

    // Convert the target container length to a tiling size.
    let tiling_size = LengthValue::from_px(target_container_length)
      .to_percentage(parent_length);

    // Skip the resize if the window is already at the target size.
    if container_to_resize.tiling_size() - tiling_size != 0. {
      resize_tiling_container(&container_to_resize, tiling_size);

      state
        .pending_sync
        .queue_containers_to_redraw(parent.tiling_children());
    }
  }

  Ok(())
}

fn tab_bar_inset_between(
  window: &TilingWindow,
  container_to_resize: &crate::models::TilingContainer,
) -> anyhow::Result<i32> {
  let mut current: Container = window.clone().into();
  let mut inset = 0_i32;

  while current.id() != container_to_resize.id() {
    let parent = current
      .parent()
      .context("Resize target is not an ancestor of the window.")?;

    if let Some(tabbed) = parent.as_tabbed() {
      inset = inset.saturating_add(tabbed.tab_bar_height());
    }

    current = parent;
  }

  Ok(inset)
}

fn set_floating_window_size(
  window: &NonTilingWindow,
  target_width: Option<LengthValue>,
  target_height: Option<LengthValue>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let monitor = window.monitor().context("No monitor")?;
  let monitor_rect = monitor.to_rect()?;
  let window_rect = window.to_rect()?;

  // Prevent resize from making the window smaller than minimum dimensions.
  // Always allow the size to be increased, even if the window would still
  // be within minimum dimension values.
  let length_with_clamp =
    |target_length: Option<i32>, current_length, min_length| {
      target_length.map_or(current_length, |target_length| {
        if target_length >= current_length {
          target_length
        } else {
          target_length.max(min_length)
        }
      })
    };

  let target_width_px = target_width
    .map(|target_width| target_width.to_px(monitor_rect.width(), None));

  let new_width = length_with_clamp(
    target_width_px,
    window_rect.width(),
    MIN_FLOATING_WIDTH,
  );

  let target_height_px = target_height
    .map(|target_height| target_height.to_px(monitor_rect.height(), None));

  let new_height = length_with_clamp(
    target_height_px,
    window_rect.height(),
    MIN_FLOATING_HEIGHT,
  );

  window.set_floating_placement(Rect::from_xy(
    window.floating_placement().x(),
    window.floating_placement().y(),
    new_width,
    new_height,
  ));

  state.pending_sync.queue_container_to_redraw(window.clone());

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::models::TabbedContainer;

  #[test]
  fn height_resize_includes_nested_tab_bars() {
    let window = TilingWindow::mock().call();
    let inner = TabbedContainer::mock()
      .tiling_containers(vec![window.clone().into()])
      .call();
    let outer = TabbedContainer::mock()
      .tiling_containers(vec![inner.into()])
      .call();

    assert_eq!(
      tab_bar_inset_between(&window, &outer.clone().into()).unwrap(),
      outer.tab_bar_height() * 2
    );
  }
}
