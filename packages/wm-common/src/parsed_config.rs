use serde::{Deserialize, Serialize};
use wm_platform::{
  Color, CornerStyle, Key, Keybinding, LengthValue, OpacityValue,
  RectDelta,
};

use crate::app_command::InvokeCommand;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct ParsedConfig {
  pub binding_modes: Vec<BindingModeConfig>,
  pub gaps: GapsConfig,
  pub general: GeneralConfig,
  pub keybindings: Vec<KeybindingConfig>,
  #[serde(alias = "padding", alias = "side_padding")]
  pub side_areas: SideAreasConfig,
  pub window_behavior: WindowBehaviorConfig,
  pub window_effects: WindowEffectsConfig,
  pub window_rules: Vec<WindowRuleConfig>,
  pub workspaces: Vec<WorkspaceConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct BindingModeConfig {
  /// Name of the binding mode.
  pub name: String,

  /// Display name of the binding mode.
  #[serde(default)]
  pub display_name: Option<String>,

  /// Keybindings that will be active when the binding mode is active.
  #[serde(default)]
  pub keybindings: Vec<KeybindingConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct GapsConfig {
  /// Whether to scale the gaps with the DPI of the monitor.
  pub scale_with_dpi: bool,

  /// Gap between adjacent windows.
  pub inner_gap: LengthValue,

  /// Gap between windows and the screen edge.
  pub outer_gap: RectDelta,

  /// Gap between window and the screen edge if there is only one window
  /// in the workspace
  pub single_window_outer_gap: Option<RectDelta>,
}

impl Default for GapsConfig {
  fn default() -> Self {
    GapsConfig {
      scale_with_dpi: true,
      inner_gap: LengthValue::from_px(0),
      outer_gap: RectDelta::new(
        LengthValue::from_px(0),
        LengthValue::from_px(0),
        LengthValue::from_px(0),
        LengthValue::from_px(0),
      ),
      single_window_outer_gap: None,
    }
  }
}

/// Configures monitor-local areas that stay visible across workspace
/// switches.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct SideAreasConfig {
  /// Whether pixel widths should scale with the monitor DPI.
  pub scale_with_dpi: bool,

  /// Width reserved at the left edge of every monitor.
  pub left: LengthValue,

  /// Width reserved at the right edge of every monitor.
  pub right: LengthValue,

  /// Optional match conditions for monitors where side areas are enabled.
  #[serde(rename = "match")]
  pub match_monitor: Option<Vec<MonitorMatchConfig>>,
}

impl Default for SideAreasConfig {
  fn default() -> Self {
    Self {
      scale_with_dpi: true,
      left: LengthValue::from_px(0),
      right: LengthValue::from_px(0),
      match_monitor: None,
    }
  }
}

impl SideAreasConfig {
  /// Whether side areas are enabled for the given monitor properties.
  #[must_use]
  pub fn matches_monitor(
    &self,
    device_name: &str,
    hardware_id: Option<&str>,
  ) -> bool {
    self.match_monitor.as_ref().is_none_or(|match_configs| {
      match_configs.iter().any(|match_config| {
        match_config.matches_monitor(device_name, hardware_id)
      })
    })
  }
}

/// Match conditions for selecting a monitor.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
  rename_all(serialize = "camelCase"),
  try_from = "RawMonitorMatchConfig"
)]
pub struct MonitorMatchConfig {
  /// Match condition for the monitor's logical display adapter name.
  pub device_name: Option<MatchType>,

  /// Match condition for the monitor's hardware ID.
  pub hardware_id: Option<MatchType>,
}

impl MonitorMatchConfig {
  /// Whether all configured conditions match the monitor properties.
  fn matches_monitor(
    &self,
    device_name: &str,
    hardware_id: Option<&str>,
  ) -> bool {
    self
      .device_name
      .as_ref()
      .is_none_or(|matcher| matcher.is_match(device_name))
      && self.hardware_id.as_ref().is_none_or(|matcher| {
        hardware_id.is_some_and(|value| matcher.is_match(value))
      })
  }
}

#[derive(Deserialize)]
struct RawMonitorMatchConfig {
  #[serde(default)]
  device_name: Option<MatchType>,
  #[serde(default)]
  hardware_id: Option<MatchType>,
}

impl TryFrom<RawMonitorMatchConfig> for MonitorMatchConfig {
  type Error = &'static str;

  fn try_from(value: RawMonitorMatchConfig) -> Result<Self, Self::Error> {
    if value.device_name.is_none() && value.hardware_id.is_none() {
      return Err(
        "at least one of `device_name` or `hardware_id` is required",
      );
    }

    Ok(Self {
      device_name: value.device_name,
      hardware_id: value.hardware_id,
    })
  }
}

/// Identifies one of the persistent side areas on a monitor.
#[derive(
  Clone,
  Copy,
  Debug,
  Deserialize,
  Eq,
  PartialEq,
  Serialize,
  clap::ValueEnum,
)]
#[serde(rename_all = "snake_case")]
#[clap(rename_all = "snake_case")]
pub enum SideArea {
  Left,
  Right,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct GeneralConfig {
  /// Config for automatically moving the cursor.
  pub cursor_jump: CursorJumpConfig,

  /// Whether to automatically focus windows underneath the cursor.
  pub focus_follows_cursor: bool,

  /// Whether to switch back and forth between the previously focused
  /// workspace when focusing the current workspace.
  pub toggle_workspace_on_refocus: bool,

  /// Commands to run when the WM has started (e.g. to run a script or
  /// launch another application).
  pub startup_commands: Vec<InvokeCommand>,

  /// Commands to run just before the WM is shutdown.
  pub shutdown_commands: Vec<InvokeCommand>,

  /// Commands to run after the WM config has reloaded.
  pub config_reload_commands: Vec<InvokeCommand>,

  /// How windows should be hidden when switching workspaces.
  #[serde(deserialize_with = "deserialize_hide_method")]
  pub hide_method: HideMethod,

  /// Affects which windows get shown in the native Windows taskbar.
  pub show_all_in_taskbar: bool,
}

impl Default for GeneralConfig {
  fn default() -> Self {
    GeneralConfig {
      cursor_jump: CursorJumpConfig::default(),
      focus_follows_cursor: false,
      toggle_workspace_on_refocus: true,
      startup_commands: vec![],
      shutdown_commands: vec![],
      config_reload_commands: vec![],
      hide_method: {
        #[cfg(target_os = "macos")]
        {
          HideMethod::PlaceInCorner
        }
        #[cfg(not(target_os = "macos"))]
        {
          HideMethod::Cloak
        }
      },
      show_all_in_taskbar: false,
    }
  }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct CursorJumpConfig {
  /// Whether to automatically move the cursor on the specified trigger.
  pub enabled: bool,

  /// Trigger for cursor jump.
  pub trigger: CursorJumpTrigger,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorJumpTrigger {
  #[default]
  MonitorFocus,
  WindowFocus,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HideMethod {
  Hide,
  #[default]
  Cloak,
  PlaceInCorner,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct KeybindingConfig {
  /// Keyboard shortcut to trigger the keybinding.
  #[serde(
    deserialize_with = "deserialize_bindings",
    serialize_with = "serialize_bindings"
  )]
  pub bindings: Vec<Keybinding>,

  /// WM commands to run when the keybinding is triggered.
  pub commands: Vec<InvokeCommand>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowBehaviorConfig {
  /// New windows are created in this state whenever possible.
  pub initial_state: InitialWindowState,

  /// Sets the default options for when a new window is created. This also
  /// changes the defaults for when the state change commands, like
  /// `set_floating`, are used without any flags.
  pub state_defaults: WindowStateDefaultsConfig,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialWindowState {
  #[default]
  Tiling,
  Floating,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowStateDefaultsConfig {
  pub floating: FloatingStateConfig,
  pub fullscreen: FullscreenStateConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct FloatingStateConfig {
  /// Whether to center new floating windows.
  pub centered: bool,

  /// Whether to show floating windows as always on top.
  pub shown_on_top: bool,
}

impl Default for FloatingStateConfig {
  fn default() -> Self {
    FloatingStateConfig {
      centered: true,
      shown_on_top: false,
    }
  }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct FullscreenStateConfig {
  /// Whether to prefer fullscreen windows to be maximized.
  pub maximized: bool,

  /// Whether to show fullscreen windows as always on top.
  pub shown_on_top: bool,
}

impl Default for FullscreenStateConfig {
  fn default() -> Self {
    FullscreenStateConfig {
      maximized: true,
      shown_on_top: false,
    }
  }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowEffectsConfig {
  /// Visual effects to apply to the focused window.
  pub focused_window: WindowEffectConfig,

  /// Visual effects to apply to non-focused windows.
  pub other_windows: WindowEffectConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowEffectConfig {
  /// Config for optionally applying a colored border.
  pub border: BorderEffectConfig,

  /// Config for optionally hiding the title bar.
  pub hide_title_bar: HideTitleBarEffectConfig,

  /// Config for optionally changing the corner style.
  pub corner_style: CornerEffectConfig,

  /// Config for optionally applying transparency.
  pub transparency: TransparencyEffectConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct BorderEffectConfig {
  /// Whether to enable the effect.
  pub enabled: bool,

  /// Color of the window border.
  pub color: Color,
}

impl Default for BorderEffectConfig {
  fn default() -> Self {
    BorderEffectConfig {
      enabled: false,
      color: Color {
        r: 140,
        g: 190,
        b: 255,
        a: 255,
      },
    }
  }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct HideTitleBarEffectConfig {
  /// Whether to enable the effect.
  pub enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct CornerEffectConfig {
  /// Whether to enable the effect.
  pub enabled: bool,

  /// Style of the window corners.
  pub style: CornerStyle,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct TransparencyEffectConfig {
  /// Whether to enable the effect.
  pub enabled: bool,

  /// The opacity to apply.
  pub opacity: OpacityValue,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct WindowRuleConfig {
  pub commands: Vec<InvokeCommand>,

  #[serde(rename = "match")]
  pub match_window: Vec<WindowMatchConfig>,

  #[serde(default = "default_window_rule_on")]
  pub on: Vec<WindowRuleEvent>,

  #[serde(default = "default_bool::<true>")]
  pub run_once: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all(serialize = "camelCase"))]
pub struct WindowMatchConfig {
  pub window_process: Option<MatchType>,
  pub window_class: Option<MatchType>,
  pub window_title: Option<MatchType>,
}

/// Due to limitations in `serde_yaml`, we need to use an untagged enum
/// instead of a regular enum for serialization. Using a regular enum
/// causes issues with flow-style objects in YAML.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum MatchType {
  Equals { equals: String },
  Includes { includes: String },
  Regex { regex: String },
  NotEquals { not_equals: String },
  NotRegex { not_regex: String },
}

impl MatchType {
  /// Whether the given value is a match for the match type.
  #[must_use]
  pub fn is_match(&self, value: &str) -> bool {
    match self {
      MatchType::Equals { equals } => value == equals,
      MatchType::Includes { includes } => value.contains(includes),
      MatchType::Regex { regex } => {
        regex::Regex::new(regex).is_ok_and(|re| re.is_match(value))
      }
      MatchType::NotEquals { not_equals } => value != not_equals,
      MatchType::NotRegex { not_regex } => {
        regex::Regex::new(not_regex).is_ok_and(|re| !re.is_match(value))
      }
    }
  }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowRuleEvent {
  /// When a window receives native focus.
  Focus,

  /// When a window is initially managed.
  Manage,

  /// When the title of a window changes.
  TitleChange,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct WorkspaceConfig {
  pub name: String,

  #[serde(default)]
  pub display_name: Option<String>,

  #[serde(default)]
  pub bind_to_monitor: Option<u32>,

  #[serde(default = "default_bool::<false>")]
  pub keep_alive: bool,
}

/// Helper function for setting a default value for a boolean field.
const fn default_bool<const V: bool>() -> bool {
  V
}

/// Helper function for setting a default value for window rule events.
fn default_window_rule_on() -> Vec<WindowRuleEvent> {
  vec![WindowRuleEvent::Manage, WindowRuleEvent::TitleChange]
}

/// Helper function for serializing a vector of keybindings.
///
/// Returns a vector of strings (e.g. `["cmd+shift+a", "ctrl+shift+b"]`).
fn serialize_bindings<S>(
  bindings: &[Keybinding],
  serializer: S,
) -> Result<S::Ok, S::Error>
where
  S: serde::Serializer,
{
  let binding_strings: Vec<String> = bindings
    .iter()
    .map(|binding| {
      binding
        .keys()
        .iter()
        .map(|key| key.to_string().to_lowercase())
        .collect::<Vec<_>>()
        .join("+")
    })
    .collect();

  binding_strings.serialize(serializer)
}

/// Helper function for deserializing a vector of strings into keybindings.
///
/// Returns a vector of [`Keybinding`].
fn deserialize_bindings<'de, D>(
  deserializer: D,
) -> Result<Vec<Keybinding>, D::Error>
where
  D: serde::de::Deserializer<'de>,
{
  let s: Vec<&str> = serde::de::Deserialize::deserialize(deserializer)?;
  s.iter()
    .map(|keybinding_str| {
      let keys: Vec<Key> = keybinding_str
        .split('+')
        .map(|key| {
          key.trim().parse().or_else(|_| Key::try_from_literal(key))
        })
        .collect::<Result<Vec<Key>, _>>()
        .map_err(serde::de::Error::custom)?;

      Keybinding::new(keys).map_err(serde::de::Error::custom)
    })
    .collect()
}

/// Helper function for deserializing [`HideMethod`].
///
/// On macOS, [`HideMethod::Hide`] and [`HideMethod::Cloak`] are not valid
/// and are automatically converted to [`HideMethod::PlaceInCorner`].
fn deserialize_hide_method<'de, D>(
  deserializer: D,
) -> Result<HideMethod, D::Error>
where
  D: serde::de::Deserializer<'de>,
{
  // LINT: The deserialized value is ignored on macOS, but we still want
  // to produce an error for invalid values.
  #[allow(unused_variables)]
  let method = HideMethod::deserialize(deserializer)?;

  #[cfg(target_os = "macos")]
  {
    Ok(HideMethod::PlaceInCorner)
  }

  #[cfg(not(target_os = "macos"))]
  {
    Ok(method)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn side_areas_default_to_disabled() {
    let parsed = serde_yaml::from_str::<ParsedConfig>("{}").unwrap();

    assert_eq!(parsed.side_areas, SideAreasConfig::default());
  }

  #[test]
  fn parses_side_area_lengths_and_legacy_alias() {
    let parsed = serde_yaml::from_str::<ParsedConfig>(
      r"
side_padding:
  scale_with_dpi: false
  left: 25%
  right: 320px
",
    )
    .unwrap();

    assert!(!parsed.side_areas.scale_with_dpi);
    assert_eq!(parsed.side_areas.left.amount, 0.25);
    assert_eq!(parsed.side_areas.right, LengthValue::from_px(320));
  }

  #[test]
  fn side_areas_match_monitor_device_names() {
    let parsed = serde_yaml::from_str::<ParsedConfig>(
      r"
side_areas:
  match:
    - device_name: { equals: DISPLAY1 }
    - device_name: { regex: '^Studio Display$' }
",
    )
    .unwrap();

    assert!(parsed.side_areas.matches_monitor("DISPLAY1", None));
    assert!(parsed.side_areas.matches_monitor("Studio Display", None));
    assert!(!parsed.side_areas.matches_monitor("DISPLAY2", None));
  }

  #[test]
  fn parses_side_area_hardware_id_match() {
    let parsed = serde_yaml::from_str::<ParsedConfig>(
      r"
side_areas:
  left: 15%
  right: 15%
  match:
    - hardware_id: { equals: DEL439E }
",
    )
    .unwrap();

    assert!(parsed
      .side_areas
      .matches_monitor(r"\\.\DISPLAY1", Some("DEL439E")));
    assert!(!parsed
      .side_areas
      .matches_monitor(r"\\.\DISPLAY1", Some("ACR1234")));
  }

  #[test]
  fn side_area_match_fields_are_anded_and_entries_are_ored() {
    let parsed = serde_yaml::from_str::<ParsedConfig>(
      r"
side_areas:
  match:
    - device_name: { equals: '\\.\DISPLAY1' }
      hardware_id: { equals: DEL439E }
    - hardware_id: { regex: '^APP' }
",
    )
    .unwrap();

    assert!(parsed
      .side_areas
      .matches_monitor(r"\\.\DISPLAY1", Some("DEL439E")));
    assert!(!parsed
      .side_areas
      .matches_monitor(r"\\.\DISPLAY1", Some("ACR1234")));
    assert!(!parsed
      .side_areas
      .matches_monitor(r"\\.\DISPLAY2", Some("DEL439E")));
    assert!(parsed
      .side_areas
      .matches_monitor(r"\\.\DISPLAY2", Some("APP5678")));
  }

  #[test]
  fn missing_hardware_id_never_matches_hardware_condition() {
    let parsed = serde_yaml::from_str::<ParsedConfig>(
      r"
side_areas:
  match:
    - hardware_id: { not_equals: SAM0001 }
",
    )
    .unwrap();

    assert!(!parsed.side_areas.matches_monitor("DISPLAY1", None));
    assert!(parsed
      .side_areas
      .matches_monitor("DISPLAY1", Some("DEL439E")));
    assert!(!parsed
      .side_areas
      .matches_monitor("DISPLAY1", Some("SAM0001")));
  }

  #[test]
  fn hardware_id_uses_existing_match_types() {
    for (matcher, matching_id, other_id) in [
      ("{ equals: DEL439E }", "DEL439E", "ACR1234"),
      ("{ includes: '439' }", "DEL439E", "ACR1234"),
      ("{ regex: '^DEL[0-9A-Z]+$' }", "DEL439E", "ACR1234"),
      ("{ not_equals: ACR1234 }", "DEL439E", "ACR1234"),
      ("{ not_regex: '^ACR' }", "DEL439E", "ACR1234"),
    ] {
      let parsed = serde_yaml::from_str::<ParsedConfig>(&format!(
        "side_areas:\n  match:\n    - hardware_id: {matcher}\n"
      ))
      .unwrap();

      assert!(parsed
        .side_areas
        .matches_monitor("DISPLAY1", Some(matching_id)));
      assert!(!parsed
        .side_areas
        .matches_monitor("DISPLAY1", Some(other_id)));
    }
  }

  #[test]
  fn rejects_empty_side_area_monitor_match() {
    let result = serde_yaml::from_str::<ParsedConfig>(
      r"
side_areas:
  match:
    - {}
",
    );

    let error = result.unwrap_err().to_string();
    assert!(
      error.contains("at least one of `device_name` or `hardware_id`"),
      "{error}"
    );
  }

  #[test]
  fn side_areas_without_monitor_match_apply_to_every_monitor() {
    let parsed =
      serde_yaml::from_str::<ParsedConfig>("side_areas: {}").unwrap();

    assert!(parsed.side_areas.matches_monitor("DISPLAY1", None));
    assert!(parsed.side_areas.matches_monitor("Studio Display", None));
  }

  #[test]
  fn empty_side_area_monitor_match_applies_to_no_monitors() {
    let parsed = serde_yaml::from_str::<ParsedConfig>(
      r"
side_areas:
  match: []
",
    )
    .unwrap();

    assert!(!parsed.side_areas.matches_monitor("DISPLAY1", None));
  }

  #[test]
  fn rejects_invalid_side_area_monitor_match() {
    let result = serde_yaml::from_str::<ParsedConfig>(
      r"
side_areas:
  match:
    - device_name: { glob: 'DISPLAY*' }
",
    );

    let error = result.unwrap_err().to_string();
    assert!(error.contains("side_areas.match[0]"), "{error}");
  }
}
