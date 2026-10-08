//! Correct alpha compositing for gpui's translucent window.
//!
//! gpui 0.2.2 builds its Metal pipelines with the colour blended "over" correctly
//! (`src·a + dst·(1 - a)`) but the alpha blended additively: the destination alpha
//! factor is `One`, so the alpha it writes is `a + dst`, clamped at one. In an opaque
//! window the alpha channel is never looked at and nothing shows. In ours it is what
//! the window server composites against the vibrancy behind the window, and it is
//! wrong as soon as two translucent fills overlap: the window film (`--background`,
//! 0.86) under the deck's sidebar film (`--surface-muted`, 0.42) reports 1.0 where the
//! true coverage is 1 - 0.14 · 0.58 = 0.92. The window then hides the material it
//! should still let through and shows the premultiplied colour on its own, which reads
//! as a muddy grey in light and too light in dark - the stack of films the Electron
//! app composited in the browser, and so got right, came out wrong here.
//!
//! The fix is the standard premultiplied "over" for alpha, `One` -> `OneMinusSourceAlpha`
//! on the destination side. gpui sets that factor through
//! `-[MTLRenderPipelineColorAttachmentDescriptor setDestinationAlphaBlendFactor:]` while
//! it builds its pipelines, so [`install`] wraps that one setter, before the first
//! window (and with it gpui's renderer) exists. Only the exact combination gpui uses
//! for its window pipelines is rewritten - a destination factor of `One` on an
//! attachment whose source alpha factor is also `One` - and only the alpha channel
//! changes: colours, text and everything opaque render as they did. Nothing else in
//! the process builds Metal pipelines.

use std::sync::OnceLock;

use cocoa::base::{id, nil};
use objc::runtime::{
    Class, Imp, Method, Object, Sel, class_getInstanceMethod, method_setImplementation,
};
use objc::{class, msg_send, sel, sel_impl};

/// `MTLBlendFactorOne`.
const ONE: u64 = 1;
/// `MTLBlendFactorOneMinusSourceAlpha`.
const ONE_MINUS_SOURCE_ALPHA: u64 = 5;

type Setter = unsafe extern "C" fn(*mut Object, Sel, u64);

/// The setter's own implementation, which the wrapper hands every call on to.
static ORIGINAL: OnceLock<usize> = OnceLock::new();

/// The wrapper installed in place of `setDestinationAlphaBlendFactor:`.
unsafe extern "C" fn set_destination_alpha_blend_factor(this: *mut Object, sel: Sel, factor: u64) {
    let Some(&original) = ORIGINAL.get() else {
        return;
    };
    // SAFETY: `this` is the colour attachment descriptor AppKit/Metal passed in; asking
    // it for its source factor is a plain getter. `original` is the IMP this method had
    // before `install` replaced it, with exactly this signature.
    unsafe {
        let mut factor = factor;
        if factor == ONE {
            let source: u64 = msg_send![this, sourceAlphaBlendFactor];
            if source == ONE {
                factor = ONE_MINUS_SOURCE_ALPHA;
            }
        }
        let original: Setter = std::mem::transmute::<usize, Setter>(original);
        original(this, sel, factor);
    }
}

/// Puts the wrapper in place. Call once at startup, before the first window opens;
/// later calls do nothing.
pub fn install() {
    if ORIGINAL.get().is_some() {
        return;
    }
    // SAFETY: main thread, before gpui creates its renderer. The descriptor is created
    // and released here only to learn the concrete class of a colour attachment, whose
    // instance method is then looked up and replaced through the Objective-C runtime.
    // The replacement has the setter's exact signature, and the previous implementation
    // is stored before the swap so the wrapper can always forward to it.
    unsafe {
        let descriptor: id = msg_send![class!(MTLRenderPipelineDescriptor), new];
        if descriptor == nil {
            return;
        }
        let attachments: id = msg_send![descriptor, colorAttachments];
        let attachment: id = msg_send![attachments, objectAtIndexedSubscript: 0u64];
        let class: *const Class = if attachment == nil {
            std::ptr::null()
        } else {
            msg_send![attachment, class]
        };
        let method = if class.is_null() {
            std::ptr::null()
        } else {
            class_getInstanceMethod(class, sel!(setDestinationAlphaBlendFactor:))
        };
        if !method.is_null() {
            let current: Imp = objc::runtime::method_getImplementation(method);
            if ORIGINAL.set(current as usize).is_ok() {
                let wrapper: Setter = set_destination_alpha_blend_factor;
                method_setImplementation(
                    method as *mut Method,
                    std::mem::transmute::<Setter, Imp>(wrapper),
                );
            }
        }
        let _: () = msg_send![descriptor, release];
    }
}
