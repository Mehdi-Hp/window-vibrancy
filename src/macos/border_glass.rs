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

const BORDER_GLASS_KEY: &str = "WindowVibrancyBorderGlassKey";

extern "C" {
    fn objc_setAssociatedObject(
        object: *const c_void,
        key: *const c_void,
        value: *const c_void,
        policy: usize,
    );
    fn objc_getAssociatedObject(object: *const c_void, key: *const c_void) -> *mut c_void;

    fn CGPathCreateMutable() -> *mut c_void;
    fn CGPathRelease(path: *mut c_void);
    fn CGPathAddRoundedRect(
        path: *mut c_void,
        transform: *const c_void,
        rect: NSRect,
        corner_width: f64,
        corner_height: f64,
    );

    static kCAFillRuleEvenOdd: *const AnyObject;
}

/// Objective-C association policy: retain the associated object and set with atomic semantics
/// Equivalent to OBJC_ASSOCIATION_RETAIN (0x301) from objc/runtime.h
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
            border_width: 1.5,
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
    let corner_radius = options.radius.unwrap_or(0.0);

    let border_glass = if let Some(glass) =
        unsafe { <NSGlassEffectView as NSGlassEffectViewExt>::new_with_frame(container_bounds) }
    {
        unsafe {
            let mask = NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable;
            glass.set_autoresizing_mask(mask);
        }
        Retained::into_super(glass)
    } else {
        let visual = NSVisualEffectView::initWithFrame(mtm.alloc(), container_bounds);
        visual.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        visual.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
        visual.setState(AppKitVisualEffectState::Active);

        let mask = NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable;
        let view: &NSView = visual.as_ref();
        view.setAutoresizingMask(mask);

        Retained::into_super(visual)
    };

    set_border_glass_style(border_glass.as_ref(), options.variant)?;
    configure_border_glass_appearance(border_glass.as_ref(), &options)?;
    set_border_view_ignores_mouse_events(border_glass.as_ref(), true)?;

    if corner_radius > 0.0 {
        apply_corner_radius_layer(border_glass.as_ref(), corner_radius);
    }

    create_border_mask(border_glass.as_ref(), border_width, corner_radius);

    container.addSubview_positioned_relativeTo(
        border_glass.as_ref(),
        NSWindowOrderingMode::Above,
        None::<&NSView>,
    );

    unsafe {
        let key = BORDER_GLASS_KEY.as_ptr() as *const c_void;
        let ptr = Retained::as_ptr(&border_glass) as *const c_void;
        objc_setAssociatedObject(
            container as *const _ as *const c_void,
            key,
            ptr,
            OBJC_ASSOCIATION_RETAIN,
        );
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

fn create_border_mask(view: &NSView, border_width: f64, corner_radius: f64) {
    unsafe {
        let _: () = msg_send![view, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![view, layer];
        if layer.is_null() {
            return;
        }

        let bounds: NSRect = msg_send![layer, bounds];
        let w = bounds.size.width;
        let h = bounds.size.height;

        let path = CGPathCreateMutable();
        if path.is_null() {
            return;
        }

        let outer_rect = NSRect {
            origin: objc2_foundation::NSPoint { x: 0.0, y: 0.0 },
            size: objc2_foundation::NSSize { width: w, height: h },
        };
        let outer_radius = (corner_radius - border_width).max(0.0);
        CGPathAddRoundedRect(path, std::ptr::null(), outer_rect, outer_radius, outer_radius);

        let inner_rect = NSRect {
            origin: objc2_foundation::NSPoint {
                x: border_width,
                y: border_width,
            },
            size: objc2_foundation::NSSize {
                width: w - (border_width * 2.0),
                height: h - (border_width * 2.0),
            },
        };
        let inner_radius = (outer_radius - border_width).max(0.0);
        CGPathAddRoundedRect(path, std::ptr::null(), inner_rect, inner_radius, inner_radius);

        let shape_layer_class = objc2::class!(CAShapeLayer);
        let shape_layer: *mut AnyObject = msg_send![shape_layer_class, layer];
        if shape_layer.is_null() {
            CGPathRelease(path);
            return;
        }

        let _: () = msg_send![shape_layer, setFrame: bounds];
        let _: () = msg_send![shape_layer, setPath: path];
        let _: () = msg_send![shape_layer, setFillRule: kCAFillRuleEvenOdd];
        let _: () = msg_send![layer, setMask: shape_layer];

        CGPathRelease(path);
    }
}

fn configure_border_glass_appearance(
    glass: &NSView,
    options: &BorderGlassOptions,
) -> Result<(), Error> {
    unsafe {
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

pub(crate) fn is_border_glass_view(container: &NSView, view: &NSView) -> bool {
    unsafe {
        let key = BORDER_GLASS_KEY.as_ptr() as *const c_void;
        let ptr = objc_getAssociatedObject(container as *const _ as *const c_void, key);
        if ptr.is_null() {
            false
        } else {
            let border_ptr = ptr as *const NSView;
            let view_ptr = view as *const NSView;
            border_ptr == view_ptr
        }
    }
}

fn remove_existing_border_glass(container: &NSView) -> bool {
    unsafe {
        let key = BORDER_GLASS_KEY.as_ptr() as *const c_void;
        let ptr = objc_getAssociatedObject(container as *const _ as *const c_void, key);
        if ptr.is_null() {
            return false;
        }

        if let Some(border_glass) = Retained::retain(ptr as *mut NSView) {
            let view: &NSView = border_glass.as_ref();
            view.removeFromSuperview();
        }

        clear_associated_border_glass(container);
        true
    }
}

fn clear_associated_border_glass(container: &NSView) {
    unsafe {
        let key = BORDER_GLASS_KEY.as_ptr() as *const c_void;
        objc_setAssociatedObject(
            container as *const _ as *const c_void,
            key,
            std::ptr::null(),
            OBJC_ASSOCIATION_RETAIN,
        );
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
