use std::sync::Arc;

use windows::{
  core::{w, PCWSTR},
  Win32::{
    Foundation::{
      COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
    },
    Graphics::Gdi::{
      BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
      FillRect, InvalidateRect, SetBkMode, SetTextColor, PAINTSTRUCT,
      TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
      CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
      GetWindowLongPtrW, IsWindow, RegisterClassW, SetWindowLongPtrW,
      SetWindowPos, ShowWindow, CREATESTRUCTW, GWLP_USERDATA,
      HTTRANSPARENT, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE,
      SW_SHOWNOACTIVATE, WM_DESTROY, WM_ERASEBKGND, WM_MOUSEACTIVATE,
      WM_NCCREATE, WM_NCHITTEST, WM_PAINT, WM_RBUTTONDOWN, WNDCLASSW,
      WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    },
  },
};

use crate::{Dispatcher, Rect, TabBarItem, ThreadBound};

const CLASS_NAME: PCWSTR = w!("GlazeWM.TabBar");
const WINDOW_TITLE: PCWSTR = w!("GlazeWM Tab Bar");

struct TabBarWindow {
  handle: HWND,
  items: Vec<TabBarItem>,
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
          on_click,
          cursor_down_index: None,
        });

        let module = unsafe { GetModuleHandleW(None)? };
        let class = WNDCLASSW {
          lpszClassName: CLASS_NAME,
          lpfnWndProc: Some(window_proc),
          hInstance: HINSTANCE(module.0),
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
    let count = i32::try_from(window.items.len()).unwrap_or(i32::MAX);

    for (index, item) in window.items.iter().enumerate() {
      let index = i32::try_from(index).unwrap_or(i32::MAX);
      let mut item_rect = RECT {
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

      item_rect.left += 8;
      item_rect.right -= 8;
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
        &raw mut item_rect,
        windows::Win32::Graphics::Gdi::DT_LEFT
          | windows::Win32::Graphics::Gdi::DT_VCENTER
          | windows::Win32::Graphics::Gdi::DT_SINGLELINE
          | windows::Win32::Graphics::Gdi::DT_END_ELLIPSIS,
      );
    }
  }

  let _ = EndPaint(window.handle, &raw const paint);
}
