use std::{collections::HashMap, sync::Arc};

use windows::{
  core::{w, PCWSTR},
  Win32::{
    Foundation::{
      COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
    },
    Graphics::Gdi::{
      BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
      FillRect, InvalidateRect, SetBkMode, SetTextColor, HBRUSH,
      PAINTSTRUCT, TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
      CreateWindowExW, DefWindowProcW, DestroyWindow, DrawIconEx,
      GetClassLongPtrW, GetClientRect, GetWindowLongPtrW, IsWindow,
      LoadCursorW, RegisterClassW, SendMessageTimeoutW, SetWindowLongPtrW,
      SetWindowPos, ShowWindow, CREATESTRUCTW, DI_NORMAL, GCLP_HICON,
      GCLP_HICONSM, GWLP_USERDATA, HICON, HTTRANSPARENT, ICON_BIG,
      ICON_SMALL, ICON_SMALL2, IDC_ARROW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
      SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE,
      WM_DESTROY, WM_ERASEBKGND, WM_GETICON, WM_MOUSEACTIVATE,
      WM_NCCREATE, WM_NCHITTEST, WM_PAINT, WM_RBUTTONDOWN, WNDCLASSW,
      WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    },
  },
};

use crate::{Dispatcher, Rect, TabBarItem, ThreadBound, WindowId};

const CLASS_NAME: PCWSTR = w!("GlazeWM.TabBar");
const WINDOW_TITLE: PCWSTR = w!("GlazeWM Tab Bar");

struct TabBarWindow {
  handle: HWND,
  items: Vec<TabBarItem>,
  icons: HashMap<WindowId, Option<HICON>>,
  on_click: Arc<dyn Fn(String) + Send + Sync>,
  cursor_down_index: Option<usize>,
}

impl Drop for TabBarWindow {
  fn drop(&mut self) {
    if self.handle.0 != 0 && unsafe { IsWindow(self.handle).as_bool() } {
      let _ = unsafe { DestroyWindow(self.handle) };
    }
  }
}

/// Windows implementation of a native tab bar.
pub(crate) struct TabBar {
  window: Option<ThreadBound<Box<TabBarWindow>>>,
}

impl TabBar {
  pub(crate) fn new<F>(
    dispatcher: &Dispatcher,
    on_click: F,
  ) -> crate::Result<Self>
  where
    F: Fn(String) + Send + Sync + 'static,
  {
    let on_click = Arc::new(on_click);
    let window = dispatcher.dispatch_sync({
      let dispatcher = dispatcher.clone();
      move || {
        let mut window = Box::new(TabBarWindow {
          handle: HWND(0),
          items: Vec::new(),
          icons: HashMap::new(),
          on_click,
          cursor_down_index: None,
        });

        let module = unsafe { GetModuleHandleW(None)? };
        let class = WNDCLASSW {
          lpszClassName: CLASS_NAME,
          lpfnWndProc: Some(window_proc),
          hInstance: HINSTANCE(module.0),
          hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
          ..Default::default()
        };

        unsafe { RegisterClassW(&raw const class) };

        let handle = unsafe {
          CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS_NAME,
            WINDOW_TITLE,
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            class.hInstance,
            Some(std::ptr::from_mut::<TabBarWindow>(&mut *window).cast()),
          )
        };

        if handle.0 == 0 {
          return Err(crate::Error::Platform(
            "Failed to create tab bar window.".to_string(),
          ));
        }

        window.handle = handle;
        Ok(ThreadBound::new(window, dispatcher))
      }
    })??;

    Ok(Self {
      window: Some(window),
    })
  }

  pub(crate) fn update(
    &mut self,
    rect: &Rect,
    items: &[TabBarItem],
  ) -> crate::Result<()> {
    let rect = rect.clone();
    let items = items.to_vec();

    let Some(window) = self.window.as_mut() else {
      return Err(crate::Error::EventLoopStopped);
    };

    window.with_mut(move |window| {
      let window_ids = items
        .iter()
        .filter_map(|item| item.window_id)
        .collect::<Vec<_>>();
      window.icons.retain(|id, _| window_ids.contains(id));
      for window_id in window_ids {
        window
          .icons
          .entry(window_id)
          .or_insert_with(|| unsafe { icon_for_window(window_id) });
      }
      window.items = items;

      unsafe {
        SetWindowPos(
          window.handle,
          None,
          rect.x(),
          rect.y(),
          rect.width(),
          rect.height(),
          SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )?;
        InvalidateRect(window.handle, None, true);
        ShowWindow(window.handle, SW_SHOWNOACTIVATE);
      }

      Ok::<(), windows::core::Error>(())
    })??;

    Ok(())
  }

  pub(crate) fn hide(&mut self) -> crate::Result<()> {
    let Some(window) = self.window.as_ref() else {
      return Ok(());
    };

    window.with(|window| unsafe {
      ShowWindow(window.handle, SW_HIDE);
    })?;
    Ok(())
  }

  pub(crate) fn close(&mut self) -> crate::Result<()> {
    self.hide()?;
    if let Some(window) = self.window.as_ref() {
      window.with(|window| unsafe {
        DestroyWindow(window.handle)?;
        Ok::<(), windows::core::Error>(())
      })??;
    }
    self.window.take();
    Ok(())
  }
}

unsafe extern "system" fn window_proc(
  hwnd: HWND,
  msg: u32,
  wparam: WPARAM,
  lparam: LPARAM,
) -> LRESULT {
  if msg == WM_NCCREATE {
    let create = &*(lparam.0 as *const CREATESTRUCTW);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    return DefWindowProcW(hwnd, msg, wparam, lparam);
  }

  let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
  if ptr == 0 {
    return DefWindowProcW(hwnd, msg, wparam, lparam);
  }
  let window = &mut *(ptr as *mut TabBarWindow);

  match msg {
    WM_PAINT => {
      paint(window);
      LRESULT(0)
    }
    WM_ERASEBKGND => LRESULT(1),
    // Do not activate the tab bar, but still deliver the click.
    WM_MOUSEACTIVATE => LRESULT(3),
    WM_NCHITTEST => {
      if window.items.is_empty() {
        LRESULT(HTTRANSPARENT as isize)
      } else {
        DefWindowProcW(hwnd, msg, wparam, lparam)
      }
    }
    WM_RBUTTONDOWN => LRESULT(0),
    windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDOWN => {
      window.cursor_down_index = tab_index_at_point(hwnd, lparam, window);
      LRESULT(0)
    }
    windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP => {
      let cursor_up_index = tab_index_at_point(hwnd, lparam, window);
      if cursor_up_index == window.cursor_down_index {
        if let Some(index) = cursor_up_index {
          (window.on_click)(window.items[index].id.clone());
        }
      }
      window.cursor_down_index = None;
      LRESULT(0)
    }
    WM_DESTROY => {
      SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
      LRESULT(0)
    }
    _ => DefWindowProcW(hwnd, msg, wparam, lparam),
  }
}

unsafe fn tab_index_at_point(
  hwnd: HWND,
  lparam: LPARAM,
  window: &TabBarWindow,
) -> Option<usize> {
  let mut client = RECT::default();
  if GetClientRect(hwnd, &raw mut client).is_err()
    || window.items.is_empty()
  {
    return None;
  }

  #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
  let x = i32::from(lparam.0 as u16);
  let width = (client.right - client.left).max(1);
  #[allow(clippy::cast_sign_loss)]
  Some(
    ((x.max(0) as usize * window.items.len()) / width as usize)
      .min(window.items.len() - 1),
  )
}

unsafe fn paint(window: &TabBarWindow) {
  let mut paint = PAINTSTRUCT::default();
  let dc = BeginPaint(window.handle, &raw mut paint);
  let mut client = RECT::default();
  let _ = GetClientRect(window.handle, &raw mut client);

  let background = CreateSolidBrush(COLORREF(0x0020_2020));
  FillRect(dc, &raw const client, background);
  let _ = DeleteObject(background);

  if !window.items.is_empty() {
    let width = (client.right - client.left).max(1);
    let height = (client.bottom - client.top).max(0);
    let count = i32::try_from(window.items.len()).unwrap_or(i32::MAX);

    for (index, item) in window.items.iter().enumerate() {
      let index = i32::try_from(index).unwrap_or(i32::MAX);
      let item_rect = RECT {
        left: client.left + width * index / count,
        top: client.top,
        right: client.left + width * (index + 1) / count,
        bottom: client.bottom,
      };

      let color = if item.is_active {
        COLORREF(0x0050_78A0)
      } else {
        COLORREF(0x0030_3030)
      };
      let brush = CreateSolidBrush(color);
      FillRect(dc, &raw const item_rect, brush);
      let _ = DeleteObject(brush);

      if index + 1 < count {
        let separator_width = (height / 24).max(1);
        let separator_margin = (height / 6).max(2);
        let separator_rect = RECT {
          left: item_rect.right - separator_width,
          top: item_rect.top + separator_margin,
          right: item_rect.right,
          bottom: item_rect.bottom - separator_margin,
        };
        let separator_brush = CreateSolidBrush(COLORREF(0x0070_7070));
        FillRect(dc, &raw const separator_rect, separator_brush);
        let _ = DeleteObject(separator_brush);
      }

      let horizontal_padding = (height / 3).clamp(6, 12);
      let mut content_rect = RECT {
        left: item_rect.left + horizontal_padding,
        top: item_rect.top,
        right: item_rect.right - horizontal_padding,
        bottom: item_rect.bottom,
      };

      let icon = item
        .window_id
        .and_then(|window_id| window.icons.get(&window_id))
        .copied()
        .flatten();
      if let Some(icon) = icon {
        let icon_size = height * 2 / 3;
        if icon_size > 0
          && content_rect.right - content_rect.left >= icon_size
        {
          let icon_top = item_rect.top + (height - icon_size) / 2;
          let _ = DrawIconEx(
            dc,
            content_rect.left,
            icon_top,
            icon,
            icon_size,
            icon_size,
            0,
            HBRUSH(0),
            DI_NORMAL,
          );
          content_rect.left += icon_size + (horizontal_padding / 2).max(3);
        }
      }

      SetBkMode(dc, TRANSPARENT);
      let text_color = if item.is_active {
        COLORREF(0x00FF_FFFF)
      } else {
        COLORREF(0x00C8_C8C8)
      };
      SetTextColor(dc, text_color);

      let mut title =
        item.title.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
      let title_len = title.len().saturating_sub(1);
      DrawTextW(
        dc,
        &mut title[..title_len],
        &raw mut content_rect,
        windows::Win32::Graphics::Gdi::DT_LEFT
          | windows::Win32::Graphics::Gdi::DT_VCENTER
          | windows::Win32::Graphics::Gdi::DT_SINGLELINE
          | windows::Win32::Graphics::Gdi::DT_END_ELLIPSIS,
      );
    }
  }

  let _ = EndPaint(window.handle, &raw const paint);
}

unsafe fn icon_for_window(window_id: WindowId) -> Option<HICON> {
  let hwnd = HWND(window_id.0);
  if hwnd.0 == 0 || !IsWindow(hwnd).as_bool() {
    return None;
  }

  for icon_type in [ICON_SMALL2, ICON_SMALL, ICON_BIG] {
    let mut icon = 0;
    let result = SendMessageTimeoutW(
      hwnd,
      WM_GETICON,
      WPARAM(icon_type as usize),
      LPARAM(0),
      SMTO_ABORTIFHUNG | SMTO_BLOCK,
      50,
      Some(&raw mut icon),
    );
    if result.0 != 0 && icon != 0 {
      return Some(HICON(icon.cast_signed()));
    }
  }

  [GCLP_HICONSM, GCLP_HICON]
    .into_iter()
    .map(|index| GetClassLongPtrW(hwnd, index))
    .find(|icon| *icon != 0)
    .map(|icon| HICON(icon.cast_signed()))
}
