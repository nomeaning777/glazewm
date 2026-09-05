use std::{
  cell::{Ref, RefCell, RefMut},
  collections::VecDeque,
  rc::Rc,
};

use anyhow::Context;
use uuid::Uuid;
use wm_common::{ContainerDto, MonitorDto, SideArea};
use wm_platform::{Display, Rect};

use crate::{
  impl_common_getters, impl_container_debug,
  models::{
    Container, DirectionContainer, NativeMonitorProperties,
    TilingContainer, WindowContainer, Workspace,
  },
  traits::{CommonGetters, PositionGetters},
};

#[derive(Clone)]
pub struct Monitor(Rc<RefCell<MonitorInner>>);

struct MonitorInner {
  id: Uuid,
  parent: Option<Container>,
  children: VecDeque<Container>,
  child_focus_order: VecDeque<Uuid>,
  native: Display,
  native_properties: NativeMonitorProperties,
}

impl Monitor {
  pub fn new(
    native_display: Display,
    native_properties: NativeMonitorProperties,
  ) -> Self {
    let monitor = MonitorInner {
      id: Uuid::new_v4(),
      parent: None,
      children: VecDeque::new(),
      child_focus_order: VecDeque::new(),
      native: native_display,
      native_properties,
    };

    Self(Rc::new(RefCell::new(monitor)))
  }

  pub fn native(&self) -> Display {
    self.0.borrow().native.clone()
  }

  pub fn set_native(&self, native: Display) {
    self.0.borrow_mut().native = native;
  }

  pub fn native_properties(&self) -> NativeMonitorProperties {
    self.0.borrow().native_properties.clone()
  }

  pub fn set_native_properties(
    &self,
    native_properties: NativeMonitorProperties,
  ) {
    self.0.borrow_mut().native_properties = native_properties;
  }

  pub fn displayed_workspace(&self) -> Option<Workspace> {
    self
      .child_focus_order()
      .filter_map(|child| child.as_workspace().cloned())
      .find(|workspace| !workspace.is_side_area())
  }

  /// Gets a persistent side area on this monitor.
  pub fn side_area(&self, side: SideArea) -> Option<Workspace> {
    self
      .children()
      .into_iter()
      .filter_map(|container| container.as_workspace().cloned())
      .find(|workspace| workspace.side_area() == Some(side))
  }

  /// Whether either persistent side area is enabled on this monitor.
  pub fn has_side_areas(&self) -> bool {
    self.side_area(SideArea::Left).is_some()
      || self.side_area(SideArea::Right).is_some()
  }

  /// Gets the effective width of a side area without allowing the two
  /// areas to overlap.
  pub fn resolved_side_area_width(
    &self,
    side: SideArea,
  ) -> anyhow::Result<i32> {
    let available_width = self
      .side_area(SideArea::Left)
      .or_else(|| self.side_area(SideArea::Right))
      .map_or_else(
        || Ok(self.native_properties().working_area.width().max(0)),
        |area| area.side_area_bounds().map(|rect| rect.width().max(0)),
      )?;
    let left = self
      .side_area(SideArea::Left)
      .map_or(Ok(0), |area| area.configured_side_area_width())?;
    let right = self
      .side_area(SideArea::Right)
      .map_or(Ok(0), |area| area.configured_side_area_width())?;
    let total = i64::from(left) + i64::from(right);

    if total <= i64::from(available_width) || total == 0 {
      return Ok(match side {
        SideArea::Left => left,
        SideArea::Right => right,
      });
    }

    let scaled_left =
      i32::try_from(i64::from(left) * i64::from(available_width) / total)
        .context("Resolved side-area width exceeded i32 bounds.")?;

    Ok(match side {
      SideArea::Left => scaled_left,
      SideArea::Right => available_width - scaled_left,
    })
  }

  pub fn workspaces(&self) -> Vec<Workspace> {
    self
      .children()
      .into_iter()
      .filter_map(|container| container.as_workspace().cloned())
      .filter(|workspace| !workspace.is_side_area())
      .collect()
  }

  /// Whether there is a difference in DPI between this monitor and the
  /// parent monitor of another container.
  pub fn has_dpi_difference(
    &self,
    other: &Container,
  ) -> anyhow::Result<bool> {
    let dpi = self.native_properties().dpi;

    let other_dpi = other
      .monitor()
      .map(|monitor| monitor.native_properties().dpi)
      .context("Failed to get DPI of other monitor.")?;

    Ok(dpi != other_dpi)
  }

  pub fn to_dto(&self) -> anyhow::Result<ContainerDto> {
    let rect = self.to_rect()?;
    let children = self
      .children()
      .iter()
      .map(CommonGetters::to_dto)
      .try_collect()?;

    Ok(ContainerDto::Monitor(MonitorDto {
      id: self.id(),
      parent_id: self.parent().map(|parent| parent.id()),
      children,
      child_focus_order: self.0.borrow().child_focus_order.clone().into(),
      has_focus: self.has_focus(None),
      width: rect.width(),
      height: rect.height(),
      x: rect.x(),
      y: rect.y(),
      dpi: self.native_properties().dpi,
      scale_factor: self.native_properties().scale_factor,
      #[cfg(target_os = "windows")]
      handle: Some(self.native_properties().handle),
      #[cfg(not(target_os = "windows"))]
      handle: None,
      device_name: self.native_properties().device_name,
      #[cfg(target_os = "windows")]
      device_path: self.native_properties().device_path,
      #[cfg(not(target_os = "windows"))]
      device_path: None,
      hardware_id: self.native_properties().hardware_id,
      working_rect: self.native_properties().working_area,
    }))
  }
}

impl_container_debug!(Monitor);
impl_common_getters!(Monitor);

impl PositionGetters for Monitor {
  fn to_rect(&self) -> anyhow::Result<Rect> {
    Ok(self.0.borrow().native_properties.bounds.clone())
  }
}

impl std::fmt::Display for Monitor {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(
      f,
      "Monitor(device_name={})",
      self.native_properties().device_name,
    )
  }
}
