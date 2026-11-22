//! The macOS readings the DAEMON takes in-process, through IOKit and
//! CoreFoundation directly — a handful of declarations rather than a
//! binding crate — where it used to spawn `ioreg`: whether you are idle and
//! whether the screen is locked (`away`), and what powers the Mac (`power`).
//! Every object the system hands back is owned here and released on drop.
//! macOS only; `away` and `power` answer for other systems themselves.

use std::ffi::{c_char, c_void, CStr};
use std::time::Duration;

type CFTypeRef = *const c_void;
type CFIndex = isize;
type CFTypeID = usize;
type IoObject = u32;
/// `kIOMainPortDefault`: the default port, spelled as null.
const MAIN_PORT_DEFAULT: u32 = 0;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const CF_NUMBER_SINT64_TYPE: CFIndex = 4;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> IoObject;
    fn IORegistryGetRootEntry(main_port: u32) -> IoObject;
    fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: CFTypeRef,
        allocator: CFTypeRef,
        options: u32,
    ) -> CFTypeRef;
    fn IOObjectRelease(object: IoObject) -> i32;
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSGetProvidingPowerSourceType(snapshot: CFTypeRef) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(alloc: CFTypeRef, cstr: *const c_char, encoding: u32)
        -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    fn CFNumberGetTypeID() -> CFTypeID;
    fn CFNumberGetValue(number: CFTypeRef, the_type: CFIndex, value: *mut c_void) -> u8;
    fn CFBooleanGetTypeID() -> CFTypeID;
    fn CFBooleanGetValue(boolean: CFTypeRef) -> u8;
    fn CFDataGetTypeID() -> CFTypeID;
    fn CFDataGetLength(data: CFTypeRef) -> CFIndex;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFArrayGetTypeID() -> CFTypeID;
    fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
    fn CFDictionaryGetTypeID() -> CFTypeID;
    fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
}

/// A CoreFoundation object this code owns (a Create/Copy result).
struct Owned(CFTypeRef);

impl Owned {
    fn new(cf: CFTypeRef) -> Option<Self> {
        (!cf.is_null()).then_some(Self(cf))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: non-null and owned, released once.
        unsafe { CFRelease(self.0) }
    }
}

/// An I/O Registry object this code owns.
struct Entry(IoObject);

impl Entry {
    fn new(object: IoObject) -> Option<Self> {
        (object != 0).then_some(Self(object))
    }

    /// The property `key`, as the registry copies it out.
    fn property(&self, key: &CStr) -> Option<Owned> {
        let key = cf_string(key)?;
        // SAFETY: a live entry and a live key; the result is owned.
        Owned::new(unsafe { IORegistryEntryCreateCFProperty(self.0, key.0, std::ptr::null(), 0) })
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        // SAFETY: a non-zero object this code was handed, released once.
        unsafe { IOObjectRelease(self.0) };
    }
}

fn cf_string(text: &CStr) -> Option<Owned> {
    // SAFETY: a NUL-terminated string; the result is owned.
    Owned::new(unsafe {
        CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), CF_STRING_ENCODING_UTF8)
    })
}

fn is(cf: CFTypeRef, type_id: CFTypeID) -> bool {
    // SAFETY: `cf` is a live CF object.
    !cf.is_null() && unsafe { CFGetTypeID(cf) } == type_id
}

fn boolean(cf: CFTypeRef) -> Option<bool> {
    // SAFETY: type-checked before the read.
    unsafe { is(cf, CFBooleanGetTypeID()).then(|| CFBooleanGetValue(cf) != 0) }
}

/// Nanoseconds since the last key or mouse movement. `HIDIdleTime` is
/// a number on every current release, raw bytes on some older ones.
pub(crate) fn hid_idle() -> Option<Duration> {
    // SAFETY: `IOServiceMatching` returns a dictionary the next call
    // consumes; a null one is never passed on.
    let service = unsafe {
        let matching = IOServiceMatching(c"IOHIDSystem".as_ptr());
        if matching.is_null() {
            return None;
        }
        Entry::new(IOServiceGetMatchingService(MAIN_PORT_DEFAULT, matching))?
    };
    let value = service.property(c"HIDIdleTime")?;
    number_or_bytes(value.0).map(Duration::from_nanos)
}

/// A 64-bit count from a CFNumber, or from a CFData of exactly eight
/// bytes in the machine's order; None for anything else.
fn number_or_bytes(cf: CFTypeRef) -> Option<u64> {
    // SAFETY: each read is type-checked first, and a data blob is read
    // only when it holds exactly eight bytes.
    unsafe {
        if is(cf, CFNumberGetTypeID()) {
            let mut n: i64 = 0;
            let read =
                CFNumberGetValue(cf, CF_NUMBER_SINT64_TYPE, (&mut n as *mut i64).cast()) != 0;
            return read.then(|| u64::try_from(n).ok()).flatten();
        }
        if is(cf, CFDataGetTypeID()) && CFDataGetLength(cf) == 8 {
            let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(cf), 8);
            return Some(u64::from_ne_bytes(bytes.try_into().ok()?));
        }
        None
    }
}

/// Locked when the root's `IOConsoleLocked`, or any console user's
/// `CGSSessionScreenIsLocked`, says so; None when neither is there.
pub(crate) fn screen_locked() -> Option<bool> {
    // SAFETY: the root entry is owned and released on drop.
    let root = Entry::new(unsafe { IORegistryGetRootEntry(MAIN_PORT_DEFAULT) })?;
    let mut flags: Vec<bool> = Vec::new();
    if let Some(locked) = root.property(c"IOConsoleLocked") {
        flags.extend(boolean(locked.0));
    }
    if let Some(users) = root.property(c"IOConsoleUsers") {
        let key = cf_string(c"CGSSessionScreenIsLocked")?;
        // SAFETY: the array and each dictionary are type-checked, and
        // the values they hold are borrowed for as long as `users`
        // lives.
        unsafe {
            if is(users.0, CFArrayGetTypeID()) {
                for i in 0..CFArrayGetCount(users.0) {
                    let user = CFArrayGetValueAtIndex(users.0, i);
                    if is(user, CFDictionaryGetTypeID()) {
                        flags.extend(boolean(CFDictionaryGetValue(user, key.0)));
                    }
                }
            }
        }
    }
    (!flags.is_empty()).then(|| flags.contains(&true))
}

/// Whether the Mac runs on its battery now: the source providing power,
/// as `IOPSGetProvidingPowerSourceType` names it, is `Battery Power`
/// (`kIOPMBatteryPowerKey`). None when the power sources can't be read.
pub(crate) fn on_battery() -> Option<bool> {
    // SAFETY: the snapshot is owned and outlives the borrowed type string
    // read off it; the comparison string is owned.
    unsafe {
        let snapshot = Owned::new(IOPSCopyPowerSourcesInfo())?;
        let providing = IOPSGetProvidingPowerSourceType(snapshot.0);
        if providing.is_null() {
            return None;
        }
        let battery = cf_string(c"Battery Power")?;
        Some(CFEqual(providing, battery.0) != 0)
    }
}
