//! Test utilities for creating mock container instances.
//!
//! This module provides default values and helper functions used by the
//! mock builders in the model modules.

use std::collections::HashSet;

use bon::bon;
use tokio::sync::mpsc;
use wm_common::{
  FloatingStateConfig, GapsConfig, SideArea, TilingDirection, WindowState,
  WorkspaceConfig,
};
use wm_platform::{
  Dispatcher, Display, LengthValue, NativeWindow, Rect, RectDelta,
};

use crate::{
  commands::container::attach_container,
  models::{
    Container, Monitor, NativeMonitorProperties, NativeWindowProperties,
    NonTilingWindow, SplitContainer, TabbedContainer, TilingContainer,
    TilingWindow, Workspace,
  },
  traits::{CommonGetters, TilingSizeGetters},
  wm_state::WmState,
};

pub const MOCK_MONITOR_WIDTH: i32 = 1680;
pub const MOCK_MONITOR_HEIGHT: i32 = 1050;
pub const MOCK_TASKBAR_HEIGHT: i32 = 50;
pub const MOCK_DPI: u32 = 96;
pub const MOCK_SCALE_FACTOR: f32 = 1.0;
pub const MOCK_WINDOW_WIDTH: i32 = 300;
pub const MOCK_WINDOW_HEIGHT: i32 = 200;

/// Creates a test state with the supplied monitors attached to its root.
pub fn state_with_monitors(monitors: Vec<Monitor>) -> WmState {
  let (event_tx, _event_rx) = mpsc::unbounded_channel();
  let (exit_tx, _exit_rx) = mpsc::unbounded_channel();
  let state = WmState::new(Dispatcher::mock(), event_tx, exit_tx);
  for monitor in monitors {
    attach_container(
      &monitor.into(),
      &state.root_container.clone().into(),
      None,
    )
    .unwrap();
  }
  state
}

/// Creates a side area containing a leaf and a nested split in either
/// layout order, with focus order differing from layout order.
pub fn mixed_side_area(
  side: SideArea,
  split_first: bool,
) -> (Workspace, SplitContainer, Vec<TilingWindow>) {
  let leaf = TilingWindow::mock().title("leaf".to_string()).call();
  let nested_first = TilingWindow::mock()
    .title("nested-first".to_string())
    .call();
  let nested_second = TilingWindow::mock()
    .title("nested-second".to_string())
    .call();
  let split = SplitContainer::mock()
    .tiling_direction(TilingDirection::Horizontal)
    .tiling_containers(vec![
      nested_first.clone().into(),
      nested_second.clone().into(),
    ])
    .call();
  split.borrow_child_focus_order_mut().swap(0, 1);

  let children = if split_first {
    vec![split.clone().into(), leaf.clone().into()]
  } else {
    vec![leaf.clone().into(), split.clone().into()]
  };
  let area = Workspace::mock_side_area()
    .side(side)
    .tiling_containers(children)
    .call();
  area.borrow_child_focus_order_mut().swap(0, 1);

  (area, split, vec![leaf, nested_first, nested_second])
}

/// Asserts that every child has the expected parent and that each raw
/// focus-order ID identifies exactly one current child.
pub fn assert_tree_links_and_focus_order(container: &Container) {
  let children = container.children();
  let child_ids = children
    .iter()
    .map(CommonGetters::id)
    .collect::<HashSet<_>>();
  let focus_ids = container
    .borrow_child_focus_order()
    .iter()
    .copied()
    .collect::<HashSet<_>>();

  assert_eq!(container.borrow_child_focus_order().len(), children.len());
  assert_eq!(focus_ids, child_ids);

  for child in children {
    assert_eq!(
      child.parent().map(|parent| parent.id()),
      Some(container.id())
    );
    assert_tree_links_and_focus_order(&child);
  }
}

pub fn mock_bounds() -> Rect {
  Rect::from_xy(0, 0, MOCK_MONITOR_WIDTH, MOCK_MONITOR_HEIGHT)
}

pub fn mock_working_area() -> Rect {
  Rect::from_xy(
    0,
    0,
    MOCK_MONITOR_WIDTH,
    MOCK_MONITOR_HEIGHT - MOCK_TASKBAR_HEIGHT,
  )
}

pub fn mock_window_rect() -> Rect {
  Rect::from_xy(0, 0, MOCK_WINDOW_WIDTH, MOCK_WINDOW_HEIGHT)
}

pub fn mock_border_delta() -> RectDelta {
  RectDelta::zero()
}

#[bon]
impl Monitor {
  #[builder]
  pub fn mock(
    #[builder(default = String::new())] device_name: String,
    #[builder(default = mock_bounds())] bounds: Rect,
    #[builder(default = mock_working_area())] working_area: Rect,
    #[builder(default = MOCK_DPI)] dpi: u32,
    #[builder(default = MOCK_SCALE_FACTOR)] scale_factor: f32,
    #[builder(default = Display::mock())] native: Display,
    #[builder(default = vec![])] workspaces: Vec<Workspace>,
  ) -> Self {
    let properties = NativeMonitorProperties::mock()
      .device_name(device_name)
      .bounds(bounds)
      .working_area(working_area)
      .dpi(dpi)
      .scale_factor(scale_factor)
      .call();

    let monitor = Self::new(native, properties);

    for workspace in workspaces {
      attach_container(&workspace.into(), &monitor.clone().into(), None)
        .unwrap();
    }

    monitor
  }
}

#[bon]
impl NativeMonitorProperties {
  #[builder]
  pub fn mock(
    #[builder(default = String::new())] device_name: String,
    #[builder(default = mock_bounds())] bounds: Rect,
    #[builder(default = mock_working_area())] working_area: Rect,
    #[builder(default = MOCK_DPI)] dpi: u32,
    #[builder(default = MOCK_SCALE_FACTOR)] scale_factor: f32,
  ) -> Self {
    Self {
      device_name,
      bounds,
      working_area,
      dpi,
      scale_factor,
      #[cfg(target_os = "macos")]
      device_uuid: String::new(),
      #[cfg(target_os = "windows")]
      handle: 0,
      #[cfg(target_os = "windows")]
      hardware_id: None,
      #[cfg(target_os = "windows")]
      device_path: None,
    }
  }
}

#[bon]
impl NativeWindowProperties {
  #[builder]
  pub fn mock(
    #[builder(default = String::new())] title: String,
    #[builder(default = String::new())] process_name: String,
    #[builder(default = mock_window_rect())] frame: Rect,
    #[builder(default = false)] is_minimized: bool,
    #[builder(default = false)] is_maximized: bool,
    #[builder(default = true)] is_resizable: bool,
  ) -> Self {
    Self {
      title,
      process_name,
      frame,
      is_minimized,
      is_maximized,
      is_resizable,
      #[cfg(target_os = "windows")]
      class_name: String::new(),
      #[cfg(target_os = "windows")]
      shadow_borders: mock_border_delta(),
    }
  }
}

#[bon]
impl NonTilingWindow {
  #[builder]
  pub fn mock(
    #[builder(default = String::new())] title: String,
    #[builder(default = String::new())] process_name: String,
    #[builder(default = mock_window_rect())] floating_placement: Rect,
    #[builder(default = WindowState::Floating(FloatingStateConfig::default()))]
    state: WindowState,
    #[builder(default = NativeWindow::mock())] native: NativeWindow,
  ) -> Self {
    let properties = NativeWindowProperties::mock()
      .title(title)
      .process_name(process_name)
      .frame(floating_placement.clone())
      .call();

    Self::new(
      None,
      native,
      properties,
      state,
      None,
      mock_border_delta(),
      None,
      floating_placement,
      false,
      vec![],
      None,
    )
  }
}

#[bon]
impl SplitContainer {
  #[builder]
  #[allow(clippy::cast_precision_loss)]
  pub fn mock(
    #[builder(default = TilingDirection::Horizontal)]
    tiling_direction: TilingDirection,
    #[builder(default = GapsConfig::default())] gaps_config: GapsConfig,
    #[builder(default = vec![])] tiling_containers: Vec<TilingContainer>,
  ) -> Self {
    let split = Self::new(tiling_direction, gaps_config);

    for child in tiling_containers {
      attach_container(&child.into(), &split.clone().into(), None)
        .unwrap();
    }

    split
  }
}

#[bon]
impl TabbedContainer {
  #[builder]
  pub fn mock(
    #[builder(default = GapsConfig::default())] gaps_config: GapsConfig,
    #[builder(default = vec![])] tiling_containers: Vec<TilingContainer>,
  ) -> Self {
    let tabbed = Self::new(gaps_config);

    for child in tiling_containers {
      attach_container(&child.into(), &tabbed.clone().into(), None)
        .unwrap();
    }

    tabbed
  }
}

#[bon]
impl TilingWindow {
  #[builder]
  pub fn mock(
    #[builder(default = 1.0)] tiling_size: f32,
    #[builder(default = String::new())] title: String,
    #[builder(default = String::new())] process_name: String,
    #[builder(default = mock_window_rect())] floating_placement: Rect,
    #[builder(default = GapsConfig::default())] gaps_config: GapsConfig,
    #[builder(default = NativeWindow::mock())] native: NativeWindow,
  ) -> Self {
    let properties = NativeWindowProperties::mock()
      .title(title)
      .process_name(process_name)
      .frame(floating_placement.clone())
      .call();

    let window = Self::new(
      None,
      native,
      properties,
      None,
      mock_border_delta(),
      floating_placement,
      false,
      gaps_config,
      vec![],
      None,
    );

    window.set_tiling_size(tiling_size);

    window
  }
}

#[bon]
impl Workspace {
  #[builder]
  pub fn mock(
    #[builder(default = "1".to_string())] name: String,
    display_name: Option<String>,
    #[builder(default = TilingDirection::Horizontal)]
    tiling_direction: TilingDirection,
    #[builder(default = GapsConfig::default())] gaps_config: GapsConfig,
    #[builder(default = vec![])] tiling_containers: Vec<TilingContainer>,
    #[builder(default = vec![])] non_tiling_windows: Vec<NonTilingWindow>,
  ) -> Self {
    let config = WorkspaceConfig {
      name,
      display_name,
      bind_to_monitor: None,
      keep_alive: false,
    };

    let workspace = Self::new(config, gaps_config, tiling_direction);

    for child in tiling_containers {
      attach_container(&child.into(), &workspace.clone().into(), None)
        .unwrap();
    }

    for child in non_tiling_windows {
      attach_container(&child.into(), &workspace.clone().into(), None)
        .unwrap();
    }

    workspace
  }
}

#[bon]
impl Workspace {
  /// Creates a mock persistent side area.
  #[builder]
  pub fn mock_side_area(
    #[builder(default = SideArea::Left)] side: SideArea,
    #[builder(default = LengthValue::from_px(300))] width: LengthValue,
    #[builder(default = true)] scale_with_dpi: bool,
    #[builder(default = GapsConfig::default())] gaps_config: GapsConfig,
    #[builder(default = vec![])] tiling_containers: Vec<TilingContainer>,
    #[builder(default = vec![])] non_tiling_windows: Vec<NonTilingWindow>,
  ) -> Self {
    let workspace =
      Self::new_side_area(side, width, scale_with_dpi, gaps_config);

    for child in tiling_containers {
      attach_container(&child.into(), &workspace.clone().into(), None)
        .unwrap();
    }

    for child in non_tiling_windows {
      attach_container(&child.into(), &workspace.clone().into(), None)
        .unwrap();
    }

    workspace
  }
}
