use std::{
  cell::{Ref, RefCell, RefMut},
  collections::VecDeque,
  rc::Rc,
};

use anyhow::Context;
use uuid::Uuid;
use wm_common::{
  ContainerDto, GapsConfig, SideArea, TilingDirection, WorkspaceConfig,
  WorkspaceDto, WorkspaceKind,
};
use wm_platform::{LengthValue, Rect, RectDelta};

use crate::{
  impl_common_getters, impl_container_debug,
  impl_tiling_direction_getters,
  models::{
    Container, DirectionContainer, TilingContainer, WindowContainer,
  },
  traits::{CommonGetters, PositionGetters, TilingDirectionGetters},
};

#[derive(Clone)]
pub struct Workspace(Rc<RefCell<WorkspaceInner>>);

#[derive(Debug)]
struct WorkspaceInner {
  id: Uuid,
  parent: Option<Container>,
  children: VecDeque<Container>,
  child_focus_order: VecDeque<Uuid>,
  config: WorkspaceConfig,
  gaps_config: GapsConfig,
  tiling_direction: TilingDirection,
  side_area: Option<SideArea>,
  side_area_width: LengthValue,
  side_area_scale_with_dpi: bool,
}

impl Workspace {
  pub fn new(
    config: WorkspaceConfig,
    gaps_config: GapsConfig,
    tiling_direction: TilingDirection,
  ) -> Self {
    let workspace = WorkspaceInner {
      id: Uuid::new_v4(),
      parent: None,
      children: VecDeque::new(),
      child_focus_order: VecDeque::new(),
      config,
      gaps_config,
      tiling_direction,
      side_area: None,
      side_area_width: LengthValue::from_px(0),
      side_area_scale_with_dpi: true,
    };

    Self(Rc::new(RefCell::new(workspace)))
  }

  /// Creates a persistent area at one side of a monitor.
  pub fn new_side_area(
    side: SideArea,
    width: LengthValue,
    scale_with_dpi: bool,
    gaps_config: GapsConfig,
  ) -> Self {
    let config = WorkspaceConfig {
      name: format!("__glazewm_side_area_{side:?}").to_lowercase(),
      display_name: None,
      bind_to_monitor: None,
      keep_alive: true,
    };

    let workspace =
      Self::new(config, gaps_config, TilingDirection::Vertical);
    workspace.set_side_area_config(width, scale_with_dpi);
    workspace.0.borrow_mut().side_area = Some(side);
    workspace
  }

  /// Underlying config for the workspace.
  pub fn config(&self) -> WorkspaceConfig {
    self.0.borrow().config.clone()
  }

  /// Update the underlying config for the workspace.
  pub fn set_config(&self, config: WorkspaceConfig) {
    self.0.borrow_mut().config = config;
  }

  /// Whether the workspace is currently displayed by the parent monitor.
  pub fn is_displayed(&self) -> bool {
    if self.is_side_area() {
      return self.monitor().is_some();
    }

    self
      .monitor()
      .and_then(|monitor| monitor.displayed_workspace())
      .is_some_and(|workspace| workspace.id() == self.id())
  }

  pub fn set_gaps_config(&self, gaps_config: GapsConfig) {
    self.0.borrow_mut().gaps_config = gaps_config;
  }

  /// Gets which persistent side this region occupies.
  pub fn side_area(&self) -> Option<SideArea> {
    self.0.borrow().side_area
  }

  /// Whether this is a persistent side area rather than a regular
  /// workspace.
  pub fn is_side_area(&self) -> bool {
    self.side_area().is_some()
  }

  /// Updates the configured width of this persistent side area.
  pub fn set_side_area_config(
    &self,
    width: LengthValue,
    scale_with_dpi: bool,
  ) {
    let mut inner = self.0.borrow_mut();
    inner.side_area_width = width;
    inner.side_area_scale_with_dpi = scale_with_dpi;
  }

  /// Effective outer gaps for this workspace.
  ///
  /// Uses `single_window_outer_gap` when the workspace has a single tiling
  /// window, otherwise falls back to `outer_gap`.
  pub fn outer_gaps(&self) -> RectDelta {
    if self.is_side_area() {
      return RectDelta::zero();
    }

    let is_single_window = self
      .descendants()
      .filter(Container::is_tiling_window)
      .nth(1)
      .is_none();

    let gaps_config = &self.0.borrow().gaps_config;
    let gaps = if is_single_window {
      gaps_config
        .single_window_outer_gap
        .as_ref()
        .unwrap_or(&gaps_config.outer_gap)
    } else {
      &gaps_config.outer_gap
    };

    // TODO: Should this be scaled by the monitor's DPI?
    gaps.clone()
  }

  /// Gets the bounds of a workspace with the given outer gap config.
  fn monitor_rect_with_gap_config(
    &self,
    outer_gaps: &RectDelta,
  ) -> anyhow::Result<Rect> {
    let monitor =
      self.monitor().context("Workspace has no parent monitor.")?;

    let gaps_config = &self.0.borrow().gaps_config;
    let scale_factor = if gaps_config.scale_with_dpi {
      monitor.native_properties().scale_factor
    } else {
      1.
    };

    // Get the delta between the monitor's bounds and its working area.
    let monitor_bounds = monitor.native_properties().bounds;
    let working_area_delta = monitor
      .native_properties()
      .working_area
      .delta(&monitor_bounds);

    Ok(
      monitor_bounds
        // Scale the gaps if `scale_with_dpi` is enabled. Outer gap config
        // values can be a percentage (relative to the monitor bounds), so
        // the outer gap delta needs to be applied prior to the working
        // area delta.
        .apply_delta(&outer_gaps.inverse(), Some(scale_factor))
        .apply_delta(&working_area_delta, None),
    )
  }

  /// Gets the bounds of a regular workspace with the given outer gap
  /// config.
  fn workspace_rect_with_gap_config(
    &self,
    outer_gaps: &RectDelta,
  ) -> anyhow::Result<Rect> {
    let monitor =
      self.monitor().context("Workspace has no parent monitor.")?;
    let mut rect = self.monitor_rect_with_gap_config(outer_gaps)?;

    // Outer gaps apply to the main tiling space, not to the full monitor.
    // Shift the already-inset edges by the reserved side widths.
    rect.left += monitor.resolved_side_area_width(SideArea::Left)?;
    rect.right -= monitor.resolved_side_area_width(SideArea::Right)?;

    if rect.right < rect.left {
      rect.right = rect.left;
    }

    Ok(rect)
  }

  /// Gets the padded monitor bounds shared by both side areas.
  pub(crate) fn side_area_bounds(&self) -> anyhow::Result<Rect> {
    let gaps_config = &self.0.borrow().gaps_config;
    self.monitor_rect_with_gap_config(&gaps_config.outer_gap)
  }

  /// Resolves this side area's configured width for its monitor.
  pub(crate) fn configured_side_area_width(&self) -> anyhow::Result<i32> {
    let monitor =
      self.monitor().context("Side area has no parent monitor.")?;
    let inner = self.0.borrow();
    let scale_factor = inner
      .side_area_scale_with_dpi
      .then_some(monitor.native_properties().scale_factor);
    let available_width = self.side_area_bounds()?.width().max(0);

    Ok(
      inner
        .side_area_width
        .to_px(available_width, scale_factor)
        .max(0)
        .min(available_width),
    )
  }

  /// Gets the maximum bounds of a workspace considering both `outer_gap`
  /// and `single_window_outer_gap` config values.
  pub fn max_workspace_rect(&self) -> anyhow::Result<Rect> {
    if self.is_side_area() {
      return self.to_rect();
    }

    let gaps_config = &self.0.borrow().gaps_config;

    // Get the workspace rect using `outer_gap`.
    let multi_window_rect =
      self.workspace_rect_with_gap_config(&gaps_config.outer_gap)?;

    let Some(single_gap) = &gaps_config.single_window_outer_gap else {
      return Ok(multi_window_rect);
    };

    // Get the workspace rect using `single_window_outer_gap`.
    let single_window_rect =
      self.workspace_rect_with_gap_config(single_gap)?;

    Ok(multi_window_rect.union(&single_window_rect))
  }

  pub fn to_dto(&self) -> anyhow::Result<ContainerDto> {
    let rect = self.to_rect()?;
    let config = self.config();

    let children = self
      .children()
      .iter()
      .map(CommonGetters::to_dto)
      .try_collect()?;

    Ok(ContainerDto::Workspace(WorkspaceDto {
      id: self.id(),
      name: config.name,
      display_name: config.display_name,
      parent_id: self.parent().map(|parent| parent.id()),
      children,
      child_focus_order: self.0.borrow().child_focus_order.clone().into(),
      has_focus: self.has_focus(None),
      is_displayed: self.is_displayed(),
      width: rect.width(),
      height: rect.height(),
      x: rect.x(),
      y: rect.y(),
      tiling_direction: self.tiling_direction(),
      kind: self.side_area().map_or(WorkspaceKind::Workspace, |side| {
        WorkspaceKind::SideArea { side }
      }),
    }))
  }
}

impl_container_debug!(Workspace);
impl_common_getters!(Workspace);
impl_tiling_direction_getters!(Workspace);

impl PositionGetters for Workspace {
  fn to_rect(&self) -> anyhow::Result<Rect> {
    let Some(side) = self.side_area() else {
      return self.workspace_rect_with_gap_config(&self.outer_gaps());
    };

    let monitor =
      self.monitor().context("Side area has no parent monitor.")?;
    let side_area_bounds = self.side_area_bounds()?;
    let width = monitor.resolved_side_area_width(side)?;

    Ok(match side {
      SideArea::Left => Rect::from_xy(
        side_area_bounds.left,
        side_area_bounds.top,
        width,
        side_area_bounds.height(),
      ),
      SideArea::Right => Rect::from_xy(
        side_area_bounds.right - width,
        side_area_bounds.top,
        width,
        side_area_bounds.height(),
      ),
    })
  }
}

impl std::fmt::Display for Workspace {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(
      f,
      "Workspace(name={}, tiling_direction={:?})",
      self.config().name,
      self.tiling_direction(),
    )
  }
}

#[cfg(test)]
mod tests {
  use wm_common::SideArea;
  use wm_platform::LengthValue;

  use super::*;
  use crate::{
    commands::container::{attach_container, set_focused_descendant},
    test_utils::{MOCK_MONITOR_WIDTH, MOCK_SCALE_FACTOR},
  };

  fn monitor_with_regions(
    left: i32,
    right: i32,
  ) -> (crate::models::Monitor, Workspace) {
    let left_area = Workspace::mock_side_area()
      .side(SideArea::Left)
      .width(LengthValue::from_px(left))
      .call();
    let workspace = Workspace::mock().call();
    let right_area = Workspace::mock_side_area()
      .side(SideArea::Right)
      .width(LengthValue::from_px(right))
      .call();
    let monitor = crate::models::Monitor::mock().call();

    for child in [
      left_area.into(),
      workspace.clone().into(),
      right_area.into(),
    ] {
      attach_container(&child, &monitor.clone().into(), None).unwrap();
    }

    (monitor, workspace)
  }

  #[test]
  fn persistent_regions_reduce_regular_workspace_bounds() {
    let (monitor, workspace) = monitor_with_regions(240, 360);
    let rect = workspace.to_rect().unwrap();
    let working_area = monitor.native_properties().working_area;

    assert_eq!(rect.left, working_area.left + 240);
    assert_eq!(rect.right, working_area.right - 360);
  }

  #[test]
  fn outer_gaps_are_measured_from_the_side_area_edges() {
    let (monitor, workspace) = monitor_with_regions(240, 360);
    workspace.set_gaps_config(GapsConfig {
      outer_gap: RectDelta::new(
        LengthValue::from_px(10),
        LengthValue::from_px(0),
        LengthValue::from_px(20),
        LengthValue::from_px(0),
      ),
      ..GapsConfig::default()
    });
    let rect = workspace.to_rect().unwrap();
    let working_area = monitor.native_properties().working_area;

    assert_eq!(rect.left, working_area.left + 240 + 10);
    assert_eq!(rect.right, working_area.right - 360 - 20);
  }

  #[test]
  fn side_areas_respect_outer_gaps() {
    let left_area = Workspace::mock_side_area()
      .side(SideArea::Left)
      .width(LengthValue::from_px(240))
      .gaps_config(GapsConfig {
        outer_gap: RectDelta::new(
          LengthValue::from_px(10),
          LengthValue::from_px(60),
          LengthValue::from_px(20),
          LengthValue::from_px(30),
        ),
        ..GapsConfig::default()
      })
      .call();
    let monitor = crate::models::Monitor::mock().call();
    attach_container(
      &left_area.clone().into(),
      &monitor.clone().into(),
      None,
    )
    .unwrap();

    let rect = left_area.to_rect().unwrap();
    let working_area = monitor.native_properties().working_area;

    assert_eq!(rect.left, working_area.left + 10);
    assert_eq!(rect.top, working_area.top + 60);
    assert_eq!(rect.width(), 240);
    assert_eq!(rect.bottom, working_area.bottom - 30,);
  }

  #[test]
  fn oversized_regions_are_clamped_without_overlap() {
    let (monitor, workspace) =
      monitor_with_regions(MOCK_MONITOR_WIDTH, MOCK_MONITOR_WIDTH);
    let left = monitor.side_area(SideArea::Left).unwrap();
    let right = monitor.side_area(SideArea::Right).unwrap();

    assert_eq!(
      left.to_rect().unwrap().width() + right.to_rect().unwrap().width(),
      monitor.native_properties().working_area.width()
    );
    assert_eq!(workspace.to_rect().unwrap().width(), 0);
  }

  #[test]
  fn pixel_width_can_scale_with_monitor_dpi() {
    let left_area = Workspace::mock_side_area()
      .side(SideArea::Left)
      .width(LengthValue::from_px(100))
      .scale_with_dpi(true)
      .call();
    let monitor = crate::models::Monitor::mock()
      .scale_factor(MOCK_SCALE_FACTOR * 2.0)
      .call();
    attach_container(&left_area.clone().into(), &monitor.into(), None)
      .unwrap();

    assert_eq!(left_area.to_rect().unwrap().width(), 200);
  }

  #[test]
  fn side_area_stays_displayed_across_regular_workspace_switches() {
    let side_area = Workspace::mock_side_area().call();
    let first = Workspace::mock().name("1".to_string()).call();
    let second = Workspace::mock().name("2".to_string()).call();
    let monitor = crate::models::Monitor::mock().call();

    for child in [
      side_area.clone().into(),
      first.clone().into(),
      second.clone().into(),
    ] {
      attach_container(&child, &monitor.clone().into(), None).unwrap();
    }

    set_focused_descendant(&first.clone().into(), None);
    assert_eq!(
      monitor
        .displayed_workspace()
        .map(|workspace| workspace.id()),
      Some(first.id())
    );

    set_focused_descendant(&side_area.clone().into(), None);
    assert!(side_area.is_displayed());
    assert_eq!(
      monitor
        .displayed_workspace()
        .map(|workspace| workspace.id()),
      Some(first.id())
    );

    set_focused_descendant(&second.clone().into(), None);
    assert!(side_area.is_displayed());
    assert_eq!(
      monitor
        .displayed_workspace()
        .map(|workspace| workspace.id()),
      Some(second.id())
    );
  }

  #[test]
  fn detached_side_area_is_not_displayed() {
    let side_area = Workspace::mock_side_area().call();
    assert!(!side_area.is_displayed());

    let monitor = crate::models::Monitor::mock().call();
    attach_container(
      &side_area.clone().into(),
      &monitor.clone().into(),
      None,
    )
    .unwrap();
    assert!(side_area.is_displayed());

    crate::commands::container::detach_container(side_area.clone().into())
      .unwrap();
    assert!(!side_area.is_displayed());
  }
}
