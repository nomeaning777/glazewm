use std::{
  cell::RefCell,
  sync::{Arc, Mutex},
};

use objc2::{
  define_class, msg_send, rc::Retained, runtime::AnyObject, sel,
  DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
  NSBackingStoreType, NSBezelStyle, NSButton, NSColor,
  NSFloatingWindowLevel, NSPanel, NSWindowCollectionBehavior,
  NSWindowStyleMask,
};
use objc2_core_foundation::CGRect;
use objc2_core_graphics::{CGDisplayBounds, CGMainDisplayID};
use objc2_foundation::{
  NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use crate::{Dispatcher, Rect, TabBarItem, ThreadBound};

struct TabBarTargetIvars {
  on_click: Arc<dyn Fn(String) + Send + Sync>,
  item_ids: RefCell<Vec<String>>,
}

define_class!(
  // SAFETY:
  // - `NSObject` has no subclassing requirements.
  // - `TabBarTarget` is only used on the main thread.
  #[unsafe(super = NSObject)]
  #[thread_kind = MainThreadOnly]
  #[ivars = TabBarTargetIvars]
  struct TabBarTarget;

  // SAFETY: `NSObjectProtocol` has no safety requirements.
  unsafe impl NSObjectProtocol for TabBarTarget {}

  impl TabBarTarget {
    #[unsafe(method(onClick:))]
    fn on_click(&self, sender: &NSButton) {
      let index = sender.tag();
      let item_ids = self.ivars().item_ids.borrow();
      if index >= 0 {
        if let Some(id) = item_ids.get(index.cast_unsigned()) {
          (self.ivars().on_click)(id.clone());
        }
      }
    }
  }
);

impl TabBarTarget {
  fn new(
    mtm: MainThreadMarker,
    on_click: Arc<dyn Fn(String) + Send + Sync>,
  ) -> Retained<Self> {
    let instance = Self::alloc(mtm).set_ivars(TabBarTargetIvars {
      on_click,
      item_ids: RefCell::new(Vec::new()),
    });

    // SAFETY: The signature of `NSObject`'s `init` is correct.
    unsafe { msg_send![super(instance), init] }
  }
}

struct TabBarWindow {
  window: Retained<NSPanel>,
  target: Retained<TabBarTarget>,
}

/// macOS implementation of a native tab bar.
pub(crate) struct TabBar {
  window: Arc<Mutex<ThreadBound<TabBarWindow>>>,
}

impl TabBar {
  pub(crate) fn new<F>(
    dispatcher: &Dispatcher,
    on_click: F,
  ) -> crate::Result<Self>
  where
    F: Fn(String) + Send + Sync + 'static,
  {
    let window = dispatcher.dispatch_sync({
      let dispatcher = dispatcher.clone();
      let on_click = Arc::new(on_click);
      move || {
        let mtm =
          MainThreadMarker::new().ok_or(crate::Error::NotMainThread)?;
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(1.0, 1.0));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
          NSPanel::alloc(mtm),
          frame,
          NSWindowStyleMask::Borderless
            | NSWindowStyleMask::NonactivatingPanel,
          NSBackingStoreType::Buffered,
          false,
        );

        // SAFETY: This panel is retained by `TabBarWindow`.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setLevel(NSFloatingWindowLevel);
        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setHidesOnDeactivate(false);
        panel.setHasShadow(false);
        panel.setOpaque(true);
        panel.setBackgroundColor(Some(
          &NSColor::colorWithSRGBRed_green_blue_alpha(
            0.125, 0.125, 0.125, 1.0,
          ),
        ));
        panel.setCollectionBehavior(
          NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.setMovable(false);
        panel.setMovableByWindowBackground(false);
        panel.setIgnoresMouseEvents(false);

        Ok::<_, crate::Error>(ThreadBound::new(
          TabBarWindow {
            window: panel,
            target: TabBarTarget::new(mtm, on_click),
          },
          dispatcher,
        ))
      }
    })??;

    Ok(Self {
      window: Arc::new(Mutex::new(window)),
    })
  }

  pub(crate) fn update(
    &mut self,
    rect: &Rect,
    items: &[TabBarItem],
  ) -> crate::Result<()> {
    let rect = rect.clone();
    let items = items.to_vec();
    let mut window = self.window.lock().map_err(|_| {
      crate::Error::Thread("Tab bar lock is poisoned.".to_string())
    })?;

    window.with_mut(move |bar| {
      let mtm =
        MainThreadMarker::new().ok_or(crate::Error::NotMainThread)?;
      bar.window.setFrame_display(cg_to_appkit_rect(&rect), true);

      let content = bar.window.contentView().ok_or(
        crate::Error::Platform("Tab bar has no content view.".to_string()),
      )?;
      for subview in &content.subviews() {
        subview.removeFromSuperview();
      }

      *bar.target.ivars().item_ids.borrow_mut() =
        items.iter().map(|item| item.id.clone()).collect();
      let count = items.len().max(1);
      #[allow(clippy::cast_precision_loss)]
      let item_width = f64::from(rect.width()) / count as f64;
      let item_height = f64::from(rect.height());

      for (index, item) in items.iter().enumerate() {
        let title = NSString::from_str(&item.title);
        // SAFETY: The selector signature matches `TabBarTarget::on_click`.
        let button = unsafe {
          NSButton::buttonWithTitle_target_action(
            &title,
            Some(&*bar.target as &AnyObject),
            Some(sel!(onClick:)),
            mtm,
          )
        };
        #[allow(clippy::cast_precision_loss)]
        button.setFrame(NSRect::new(
          NSPoint::new(item_width * index as f64, 0.0),
          NSSize::new(item_width, item_height),
        ));
        button.setTag(index.cast_signed());
        button.setBezelStyle(if item.is_active {
          NSBezelStyle::AccessoryBar
        } else {
          NSBezelStyle::AccessoryBarAction
        });
        button.setToolTip(Some(&title));
        content.addSubview(&button);
      }

      bar.window.orderFrontRegardless();
      Ok(())
    })?
  }

  pub(crate) fn hide(&mut self) -> crate::Result<()> {
    let window = self.window.lock().map_err(|_| {
      crate::Error::Thread("Tab bar lock is poisoned.".to_string())
    })?;
    window.with(|bar| bar.window.orderOut(None))?;
    Ok(())
  }

  pub(crate) fn close(&mut self) -> crate::Result<()> {
    let window = self.window.lock().map_err(|_| {
      crate::Error::Thread("Tab bar lock is poisoned.".to_string())
    })?;
    window.with(|bar| {
      bar.window.orderOut(None);
      bar.window.close();
    })?;
    Ok(())
  }
}

fn cg_to_appkit_rect(rect: &Rect) -> NSRect {
  let main_bounds: CGRect = CGDisplayBounds(CGMainDisplayID());
  let y = main_bounds.size.height
    - (f64::from(rect.y()) + f64::from(rect.height()));

  NSRect::new(
    NSPoint::new(f64::from(rect.x()), y),
    NSSize::new(f64::from(rect.width()), f64::from(rect.height())),
  )
}
