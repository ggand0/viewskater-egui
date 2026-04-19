//! macOS "Open With" / Finder double-click support.
//!
//! The initial `application:openFiles:` event that Finder sends when the user
//! launches a bundled app with a file is dispatched very early — before eframe
//! runs its creation-context callback (and therefore before we could normally
//! install a handler). To catch it, we register an NSNotificationCenter
//! observer for `NSApplicationWillFinishLaunchingNotification` *before*
//! `eframe::run_native`. When that notification fires during AppKit's
//! `-finishLaunching`, winit's delegate has already been installed, so we can
//! swizzle it and add our own `application:openFiles:` method in time.
//!
//! Delivered paths are forwarded through an mpsc channel that the egui update
//! loop drains into `Pane::open_path`.
//!
//! This module is intentionally narrow: no sandbox, no security-scoped
//! bookmarks. The iced version at ../viewskater/src/macos_file_access.rs has
//! the full sandboxed implementation if we ever ship through the App Store.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use objc2::declare::ClassBuilder;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject, Sel};
use objc2::{msg_send, msg_send_id, sel, ClassType};
use objc2_app_kit::NSApplication;
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDictionary, NSNotificationCenter, NSString, NSUserDefaults,
};

static FILE_CHANNEL: OnceLock<Sender<PathBuf>> = OnceLock::new();
static LAUNCH_OBSERVER: OnceLock<usize> = OnceLock::new();

pub fn set_file_channel(sender: Sender<PathBuf>) {
    let _ = FILE_CHANNEL.set(sender);
}

fn send_path(path: String) {
    log::debug!("macOS Finder opened: {path}");
    if let Some(sender) = FILE_CHANNEL.get() {
        if let Err(e) = sender.send(PathBuf::from(path)) {
            log::error!("Failed to forward opened path to UI thread: {e}");
        }
    } else {
        log::warn!("FILE_CHANNEL not set; dropping opened path");
    }
}

/// Handler for `application:openFiles:` (modern, multi-file).
unsafe extern "C" fn handle_open_files(
    _this: &mut AnyObject,
    _sel: Sel,
    _sender: &AnyObject,
    files: &NSArray<NSString>,
) {
    autoreleasepool(|pool| {
        for file in files.iter() {
            send_path(file.as_str(pool).to_owned());
        }
    });
}

/// Handler for `application:openFile:` (legacy, single file). Void return
/// matches the iced version; modern AppKit dispatches through openFiles: first.
unsafe extern "C" fn handle_open_file(
    _this: &mut AnyObject,
    _sel: Sel,
    _sender: &AnyObject,
    filename: &NSString,
) {
    autoreleasepool(|pool| {
        send_path(filename.as_str(pool).to_owned());
    });
}

/// NSNotification callback invoked when `-applicationWillFinishLaunching:`
/// fires. At this point winit's delegate exists but AppKit has not yet
/// installed its default kAEOpenDocuments handler, so swizzling here races
/// ahead of the initial `openFiles:` dispatch.
unsafe extern "C" fn on_will_finish_launching(
    _this: &mut AnyObject,
    _sel: Sel,
    _notification: &AnyObject,
) {
    swizzle_delegate();
}

/// Adds our open-file methods to the NSApplicationDelegate currently attached
/// to the shared NSApplication. Idempotent.
fn swizzle_delegate() {
    static DONE: OnceLock<()> = OnceLock::new();
    if DONE.get().is_some() {
        return;
    }

    let Some(mtm) = MainThreadMarker::new() else {
        log::error!("swizzle_delegate called off the main thread");
        return;
    };

    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let Some(delegate) = app.delegate() else {
            log::warn!("No NSApplication delegate yet; cannot swizzle");
            return;
        };

        let existing_class: &AnyClass = msg_send![&delegate, class];
        log::debug!(
            "Swizzling NSApplicationDelegate class: {}",
            existing_class.name()
        );

        let Some(mut builder) =
            ClassBuilder::new("ViewSkaterApplicationDelegate", existing_class)
        else {
            log::warn!("ViewSkaterApplicationDelegate already registered; skipping");
            return;
        };

        builder.add_method(
            sel!(application:openFiles:),
            handle_open_files as unsafe extern "C" fn(_, _, _, _),
        );
        builder.add_method(
            sel!(application:openFile:),
            handle_open_file as unsafe extern "C" fn(_, _, _, _),
        );
        let new_class = builder.register();

        let delegate_obj = Retained::cast::<AnyObject>(delegate);
        AnyObject::set_class(&delegate_obj, new_class);

        // Prevent AppKit from silently turning unknown argv entries into
        // open-document events on launch. We handle CLI args via clap.
        let key = NSString::from_str("NSTreatUnknownArgumentsAsOpen");
        let value: Retained<AnyObject> =
            Retained::cast::<AnyObject>(NSString::from_str("NO"));
        let dict = NSDictionary::from_vec(&[key.as_ref()], vec![value]);
        NSUserDefaults::standardUserDefaults().registerDefaults(dict.as_ref());
    }

    let _ = DONE.set(());
    log::debug!("NSApplicationDelegate swizzled with openFiles:/openFile:");
}

/// Registers a notification observer for `NSApplicationWillFinishLaunchingNotification`
/// so we can swizzle winit's delegate before AppKit dispatches the initial
/// open-files AppleEvent. Must be called on the main thread before
/// `eframe::run_native`.
pub fn install_launch_observer() {
    if LAUNCH_OBSERVER.get().is_some() {
        return;
    }

    unsafe {
        let class = build_observer_class();

        let observer: Retained<NSObject> = msg_send_id![class, new];
        let observer_raw: *const NSObject = Retained::as_ptr(&observer);
        // Leak the observer so it lives for the entire process — required
        // since NSNotificationCenter holds only a weak reference.
        std::mem::forget(observer);
        let _ = LAUNCH_OBSERVER.set(observer_raw as usize);

        let center = NSNotificationCenter::defaultCenter();
        let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
        let observer_obj: &AnyObject = &*(observer_raw as *const AnyObject);
        let _: () = msg_send![
            &*center,
            addObserver: observer_obj,
            selector: sel!(onWillFinishLaunching:),
            name: &*name,
            object: std::ptr::null::<AnyObject>()
        ];
    }

    log::debug!("macOS launch observer installed");
}

/// Build (or retrieve) our observer class with a single
/// `onWillFinishLaunching:` method.
unsafe fn build_observer_class() -> &'static AnyClass {
    if let Some(cls) = AnyClass::get("ViewSkaterLaunchObserver") {
        return cls;
    }

    let mut builder =
        ClassBuilder::new("ViewSkaterLaunchObserver", NSObject::class())
            .expect("Failed to create ViewSkaterLaunchObserver class");
    builder.add_method(
        sel!(onWillFinishLaunching:),
        on_will_finish_launching as unsafe extern "C" fn(_, _, _),
    );
    builder.register()
}

/// Late fallback swizzle — called from the egui creation context to cover the
/// case where the launch-observer somehow missed (e.g. notification name
/// change in a future macOS release). Already-running openFiles: events go
/// through whichever swizzle ran first; both install the same method.
pub fn register_file_handler() {
    swizzle_delegate();
}
