use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ContainerDto;
use crate::{SideArea, TilingDirection};

/// Describes the role of a workspace-like region in the container tree.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceKind {
  /// A regular switchable workspace.
  #[default]
  Workspace,

  /// A persistent monitor-local side area.
  SideArea { side: SideArea },
}

/// User-friendly representation of a workspace.
///
/// Used for IPC and debug logging.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDto {
  pub id: Uuid,
  pub name: String,
  pub display_name: Option<String>,
  pub parent_id: Option<Uuid>,
  pub children: Vec<ContainerDto>,
  pub child_focus_order: Vec<Uuid>,
  pub has_focus: bool,
  pub is_displayed: bool,
  pub width: i32,
  pub height: i32,
  pub x: i32,
  pub y: i32,
  pub tiling_direction: TilingDirection,
  #[serde(default)]
  pub kind: WorkspaceKind,
}
