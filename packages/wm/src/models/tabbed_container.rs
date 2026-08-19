use std::{
  cell::{Ref, RefCell, RefMut},
  collections::VecDeque,
  rc::Rc,
};

use anyhow::Context;
use uuid::Uuid;
use wm_common::{
  ContainerDto, GapsConfig, TabbedContainerDto, TilingDirection,
};
use wm_platform::Rect;

use crate::{
  impl_common_getters, impl_container_debug,
  impl_position_getters_as_resizable, impl_tiling_size_getters,
  models::{
    Container, DirectionContainer, TilingContainer, WindowContainer,
  },
  traits::{
    CommonGetters, PositionGetters, TilingDirectionGetters,
    TilingSizeGetters,
  },
};

/// Height reserved above tabbed content for the tab bar.
///
/// The bar is rendered by the platform UI above the tiled windows. Window
/// content is inset to keep it from overlapping the bar.
pub const TAB_BAR_HEIGHT: i32 = 24;

/// A tiling container whose children share the same content rectangle.
///
/// The child at the front of `child_focus_order` is the active tab.
#[derive(Clone)]
pub struct TabbedContainer(Rc<RefCell<TabbedContainerInner>>);

struct TabbedContainerInner {
  id: Uuid,
  parent: Option<Container>,
  children: VecDeque<Container>,
  child_focus_order: VecDeque<Uuid>,
  tiling_size: f32,
  gaps_config: GapsConfig,
}

impl TabbedContainer {
  /// Creates an empty tabbed container.
  pub fn new(gaps_config: GapsConfig) -> Self {
    Self(Rc::new(RefCell::new(TabbedContainerInner {
      id: Uuid::new_v4(),
      parent: None,
      children: VecDeque::new(),
      child_focus_order: VecDeque::new(),
      tiling_size: 1.0,
      gaps_config,
    })))
  }

  /// Gets the active direct child.
  pub fn active_child(&self) -> Option<Container> {
    self.child_focus_order().next()
  }

  /// Gets the DPI-scaled height reserved for the tab bar.
  pub fn tab_bar_height(&self) -> i32 {
    let scale_factor = self
      .monitor()
      .map_or(1.0, |monitor| monitor.native_properties().scale_factor);

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    {
      (TAB_BAR_HEIGHT as f32 * scale_factor).round() as i32
    }
  }

  /// Gets the rectangle occupied by child content, excluding the tab bar.
  pub fn content_rect(&self) -> anyhow::Result<Rect> {
    let rect = self.to_rect()?;
    let tab_bar_height = self.tab_bar_height().min(rect.height().max(0));

    Ok(Rect::from_xy(
      rect.x(),
      rect.y() + tab_bar_height,
      rect.width(),
      rect.height() - tab_bar_height,
    ))
  }

  /// Converts this container to its IPC representation.
  pub fn to_dto(&self) -> anyhow::Result<ContainerDto> {
    let rect = self.to_rect()?;
    let children = self
      .children()
      .iter()
      .map(CommonGetters::to_dto)
      .try_collect()?;

    Ok(ContainerDto::Tabbed(TabbedContainerDto {
      id: self.id(),
      parent_id: self.parent().map(|parent| parent.id()),
      children,
      child_focus_order: self.0.borrow().child_focus_order.clone().into(),
      active_child_id: self.active_child().map(|child| child.id()),
      tab_bar_height: self.tab_bar_height(),
      has_focus: self.has_focus(None),
      tiling_size: self.tiling_size(),
      width: rect.width(),
      height: rect.height(),
      x: rect.x(),
      y: rect.y(),
    }))
  }
}

impl_container_debug!(TabbedContainer);
impl_common_getters!(TabbedContainer);
impl_tiling_size_getters!(TabbedContainer);
impl_position_getters_as_resizable!(TabbedContainer);

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{
    models::{Monitor, TilingWindow, Workspace},
    traits::{CommonGetters, PositionGetters},
  };

  #[test]
  fn children_share_content_rect() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let expected_height =
      crate::test_utils::mock_working_area().height() - TAB_BAR_HEIGHT;
    let tabbed = TabbedContainer::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![tabbed.clone().into()])
      .call();
    let _monitor = Monitor::mock().workspaces(vec![workspace]).call();

    assert_eq!(first.to_rect().unwrap(), second.to_rect().unwrap());
    assert_eq!(first.to_rect().unwrap(), tabbed.content_rect().unwrap());
    assert_eq!(first.to_rect().unwrap().height(), expected_height);
  }

  #[test]
  fn only_active_tab_is_visible() {
    let first = TilingWindow::mock().call();
    let second = TilingWindow::mock().call();
    let tabbed = TabbedContainer::mock()
      .tiling_containers(vec![first.clone().into(), second.clone().into()])
      .call();

    assert!(first.is_active_tab_descendant());
    assert!(!second.is_active_tab_descendant());

    tabbed.borrow_child_focus_order_mut().swap(0, 1);

    assert!(!first.is_active_tab_descendant());
    assert!(second.is_active_tab_descendant());
  }

  #[test]
  fn nested_tabs_require_every_ancestor_active() {
    // Build `T[ inner=T[a, b], c ]`, where the outer stack's active tab is
    // the inner stack.
    let a = TilingWindow::mock().call();
    let b = TilingWindow::mock().call();
    let inner = TabbedContainer::mock()
      .tiling_containers(vec![a.clone().into(), b.clone().into()])
      .call();
    let c = TilingWindow::mock().call();
    let outer = TabbedContainer::mock()
      .tiling_containers(vec![inner.clone().into(), c.clone().into()])
      .call();
    let workspace = Workspace::mock()
      .tiling_containers(vec![outer.clone().into()])
      .call();
    let _monitor = Monitor::mock().workspaces(vec![workspace]).call();

    // `a` is active in the inner stack and the inner stack is active in
    // the outer stack, so `a` is visible.
    assert!(a.is_active_tab_descendant());
    assert!(!b.is_active_tab_descendant());
    assert!(!c.is_active_tab_descendant());

    // Switching the outer stack to `c` hides the entire inner stack.
    outer.borrow_child_focus_order_mut().swap(0, 1);
    assert!(!a.is_active_tab_descendant());
    assert!(!b.is_active_tab_descendant());
    assert!(c.is_active_tab_descendant());
  }
}
