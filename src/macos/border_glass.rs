use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, MessageReceiver};
use objc2::{msg_send, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState as AppKitVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
};
use objc2_foundation::{MainThreadMarker, NSRect};

use crate::macos::ns_glass_effect_view::{
    NSGlassEffectVariant, NSGlassEffectView, NSGlassEffectViewExt,
};
use crate::{Color, Error};

const BORDER_GLASS_TOP_KEY: &str = "WindowVibrancyBorderGlassTopKey";
const BORDER_GLASS_RIGHT_KEY: &str = "WindowVibrancyBorderGlassRightKey";
const BORDER_GLASS_BOTTOM_KEY: &str = "WindowVibrancyBorderGlassBottomKey";
const BORDER_GLASS_LEFT_KEY: &str = "WindowVibrancyBorderGlassLeftKey";

extern "C" {
    fn objc_setAssociatedObject(
        object: *const c_void,
        key: *const c_void,
        value: *const c_void,
        policy: usize,
    );
    fn objc_getAssociatedObject(object: *const c_void, key: *const c_void) -> *mut c_void;
}

const OBJC_ASSOCIATION_RETAIN: usize = 0x301;

#[derive(Debug, Clone)]
pub struct BorderGlassOptions {
    pub variant: NSGlassEffectVariant,
    pub tint: Option<Color>,
    pub radius: Option<f64>,
    pub border_width: f64,
    pub state: Option<crate::macos::NSVisualEffectState>,
}

impl Default for BorderGlassOptions {
    fn default() -> Self {
        Self {
            variant: NSGlassEffectVariant::Clear,
            tint: None,
            radius: None,
            border_width: 1.0,
            state: None,
        }
    }
}

pub unsafe fn apply_border_glass(
    ns_view: NonNull<c_void>,
    options: BorderGlassOptions,
) -> Result<(), Error> {
    let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread(
        "apply_border_glass must be called on main thread",
    ))?;

    let container: &NSView = ns_view.cast().as_ref();
    remove_existing_border_glass(container);

    let container_bounds = container.bounds();
    let border_width = options.border_width;
    let w = container_bounds.size.width;
    let h = container_bounds.size.height;

    let border_strips = [
        ("top", NSRect {
            origin: objc2_foundation::NSPoint { x: 0.0, y: h - border_width },
            size: objc2_foundation::NSSize { width: w, height: border_width },
        }, BORDER_GLASS_TOP_KEY, NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin),

        ("right", NSRect {
            origin: objc2_foundation::NSPoint { x: w - border_width, y: 0.0 },
            size: objc2_foundation::NSSize { width: border_width, height: h },
        }, BORDER_GLASS_RIGHT_KEY, NSAutoresizingMaskOptions::ViewHeightSizable | NSAutoresizingMaskOptions::ViewMinXMargin),

        ("bottom", NSRect {
            origin: objc2_foundation::NSPoint { x: 0.0, y: 0.0 },
            size: objc2_foundation::NSSize { width: w, height: border_width },
        }, BORDER_GLASS_BOTTOM_KEY, NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin),

        ("left", NSRect {
            origin: objc2_foundation::NSPoint { x: 0.0, y: 0.0 },
            size: objc2_foundation::NSSize { width: border_width, height: h },
        }, BORDER_GLASS_LEFT_KEY, NSAutoresizingMaskOptions::ViewHeightSizable | NSAutoresizingMaskOptions::ViewMaxXMargin),
    ];

    for (_edge_name, rect, key, autoresizing_mask) in border_strips.iter() {
        let border_glass = if let Some(glass) =
            unsafe { <NSGlassEffectView as NSGlassEffectViewExt>::new_with_frame(*rect) }
        {
            unsafe {
                glass.set_autoresizing_mask(*autoresizing_mask);
            }
            Retained::into_super(glass)
        } else {
            let visual = NSVisualEffectView::initWithFrame(mtm.alloc(), *rect);
            visual.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            visual.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
            visual.setState(AppKitVisualEffectState::Active);

            let view: &NSView = visual.as_ref();
            view.setAutoresizingMask(*autoresizing_mask);

            Retained::into_super(visual)
        };

        set_border_glass_style(border_glass.as_ref(), options.variant)?;
        configure_border_glass_appearance(border_glass.as_ref(), &options)?;
        set_border_view_ignores_mouse_events(border_glass.as_ref(), true)?;

        container.addSubview_positioned_relativeTo(
            border_glass.as_ref(),
            NSWindowOrderingMode::Below,
            None::<&NSView>,
        );

        unsafe {
            let key_ptr = key.as_ptr() as *const c_void;
            let ptr = Retained::as_ptr(&border_glass) as *const c_void;
            objc_setAssociatedObject(
                container as *const _ as *const c_void,
                key_ptr,
                ptr,
                OBJC_ASSOCIATION_RETAIN,
            );
        }
    }

    Ok(())
}

pub unsafe fn clear_border_glass(ns_view: NonNull<c_void>) -> Result<bool, Error> {
    let _mtm = MainThreadMarker::new().ok_or(Error::NotMainThread(
        "clear_border_glass must be called on main thread",
    ))?;

    let container: &NSView = ns_view.cast().as_ref();
    Ok(remove_existing_border_glass(container))
}

fn configure_border_glass_appearance(
    glass: &NSView,
    options: &BorderGlassOptions,
) -> Result<(), Error> {
    unsafe {
        if let Some(radius) = options.radius {
            if radius > 0.0 {
                let selector = sel!(setCornerRadius:);
                let responds_corner: bool = msg_send![glass, respondsToSelector: selector];

                if responds_corner {
                    let _: () = MessageReceiver::send_message(
                        glass as *const _ as *mut AnyObject,
                        selector,
                        (radius,),
                    );
                } else {
                    apply_corner_radius_layer(glass, radius);
                }
            }
        }

        if let Some(tint) = options.tint {
            let color = color_to_nscolor(tint);
            let selector = sel!(setTintColor:);
            let responds: bool = msg_send![glass, respondsToSelector: selector];

            if responds {
                let _: () = MessageReceiver::send_message(
                    glass as *const _ as *mut AnyObject,
                    selector,
                    (&*color,),
                );
            } else {
                let _: () = msg_send![glass, setWantsLayer: true];
                let layer: *mut AnyObject = msg_send![glass, layer];
                if !layer.is_null() {
                    let cg_color: *mut AnyObject = msg_send![&*color, CGColor];
                    let _: () = msg_send![layer, setBackgroundColor: cg_color];
                }
            }
        }
    }

    let state = options
        .state
        .map(|state| AppKitVisualEffectState(state as isize))
        .unwrap_or(AppKitVisualEffectState::FollowsWindowActiveState);
    set_border_glass_state(glass, state)?;
    Ok(())
}

fn color_to_nscolor((r, g, b, a): Color) -> Retained<objc2_app_kit::NSColor> {
    let rf = r as f64 / 255.0;
    let gf = g as f64 / 255.0;
    let bf = b as f64 / 255.0;
    let af = a as f64 / 255.0;

    objc2_app_kit::NSColor::colorWithRed_green_blue_alpha(rf, gf, bf, af)
}

unsafe fn apply_corner_radius_layer(view: &NSView, radius: f64) {
    let _: () = msg_send![view, setWantsLayer: true];
    let layer: *mut AnyObject = msg_send![view, layer];
    if !layer.is_null() {
        let _: () = msg_send![layer, setCornerRadius: radius];
        let _: () = msg_send![layer, setMasksToBounds: true];
    }
}

pub fn is_border_glass_view(container: &NSView, view: &NSView) -> bool {
    let keys = [
        BORDER_GLASS_TOP_KEY,
        BORDER_GLASS_RIGHT_KEY,
        BORDER_GLASS_BOTTOM_KEY,
        BORDER_GLASS_LEFT_KEY,
    ];

    unsafe {
        let view_ptr = view as *const NSView;
        for key in keys.iter() {
            let key_ptr = key.as_ptr() as *const c_void;
            let ptr = objc_getAssociatedObject(container as *const _ as *const c_void, key_ptr);
            if !ptr.is_null() {
                let border_ptr = ptr as *const NSView;
                if border_ptr == view_ptr {
                    return true;
                }
            }
        }
    }
    false
}

fn remove_existing_border_glass(container: &NSView) -> bool {
    let keys = [
        BORDER_GLASS_TOP_KEY,
        BORDER_GLASS_RIGHT_KEY,
        BORDER_GLASS_BOTTOM_KEY,
        BORDER_GLASS_LEFT_KEY,
    ];

    let mut had_border = false;

    for key in keys.iter() {
        unsafe {
            let key_ptr = key.as_ptr() as *const c_void;
            let ptr = objc_getAssociatedObject(container as *const _ as *const c_void, key_ptr);
            if !ptr.is_null() {
                if let Some(border_glass) = Retained::retain(ptr as *mut NSView) {
                    let view: &NSView = border_glass.as_ref();
                    view.removeFromSuperview();
                    had_border = true;
                }
            }
        }
    }

    clear_associated_border_glass(container);
    had_border
}

fn clear_associated_border_glass(container: &NSView) {
    let keys = [
        BORDER_GLASS_TOP_KEY,
        BORDER_GLASS_RIGHT_KEY,
        BORDER_GLASS_BOTTOM_KEY,
        BORDER_GLASS_LEFT_KEY,
    ];

    unsafe {
        for key in keys.iter() {
            let key_ptr = key.as_ptr() as *const c_void;
            objc_setAssociatedObject(
                container as *const _ as *const c_void,
                key_ptr,
                std::ptr::null(),
                OBJC_ASSOCIATION_RETAIN,
            );
        }
    }
}

fn set_border_glass_style(view: &NSView, style: NSGlassEffectVariant) -> Result<(), Error> {
    unsafe {
        if <NSGlassEffectView as NSGlassEffectViewExt>::is_available() {
            let ptr = view as *const NSView as *mut NSGlassEffectView;
            if !ptr.is_null() {
                if let Some(glass_view) = Retained::retain(ptr) {
                    if glass_view.set_style_variant(style) {
                        return Ok(());
                    }
                }
            }
        }

        let selectors = [sel!(setStyle:)];
        send_optional_i64(view, &selectors, style as i64)
    }
}

fn set_border_glass_state(view: &NSView, state: AppKitVisualEffectState) -> Result<(), Error> {
    unsafe {
        if <NSGlassEffectView as NSGlassEffectViewExt>::is_available() {
            let ptr = view as *const NSView as *mut NSGlassEffectView;
            if !ptr.is_null() {
                if let Some(glass_view) = Retained::retain(ptr) {
                    if glass_view.set_state(state) {
                        return Ok(());
                    }
                }
            }
        }

        let raw_state = state.0 as i64;
        let selectors = [
            objc2::runtime::Sel::register(c"set_state:"),
            sel!(setState:),
        ];
        send_optional_i64(view, &selectors, raw_state)
    }
}

fn send_optional_i64(
    view: &NSView,
    selectors: &[objc2::runtime::Sel],
    value: i64,
) -> Result<(), Error> {
    unsafe {
        for &sel in selectors {
            let responds: bool = msg_send![view, respondsToSelector: sel];
            if responds {
                let val: isize = value as isize;
                let _: () =
                    MessageReceiver::send_message(view as *const _ as *mut AnyObject, sel, (val,));
                return Ok(());
            }
        }
    }

    Ok(())
}

fn set_border_view_ignores_mouse_events(view: &NSView, ignores: bool) -> Result<(), Error> {
    unsafe {
        if <NSGlassEffectView as NSGlassEffectViewExt>::is_available() {
            let ptr = view as *const NSView as *mut NSGlassEffectView;
            if !ptr.is_null() {
                if let Some(glass_view) = Retained::retain(ptr) {
                    if glass_view.set_ignores_mouse_events(ignores) {
                        return Ok(());
                    }
                }
            }
        }

        let selectors = [
            objc2::runtime::Sel::register(c"set_ignoresMouseEvents:"),
            sel!(setIgnoresMouseEvents:),
        ];
        send_optional_bool(view, &selectors, ignores)
    }
}

fn send_optional_bool(
    view: &NSView,
    selectors: &[objc2::runtime::Sel],
    value: bool,
) -> Result<(), Error> {
    unsafe {
        for &sel in selectors {
            let responds: bool = msg_send![view, respondsToSelector: sel];
            if responds {
                let objc_value = objc2::runtime::Bool::new(value);
                let _: () = MessageReceiver::send_message(
                    view as *const _ as *mut AnyObject,
                    sel,
                    (objc_value,),
                );
                return Ok(());
            }
        }
    }

    Ok(())
}
