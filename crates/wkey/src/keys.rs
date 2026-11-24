//! Keys typed into Zed. Zed tells extensions nothing about pending vim keys,
//! so on macOS wkey listens to key presses (never changing them) and keeps
//! the ones Zed receives.
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// `kCGEventFlagMask*` (IOLLEvent.h).
const FLAGS: [(u64, KeyModifiers); 4] = [
    (0x0002_0000, KeyModifiers::SHIFT),
    (0x0004_0000, KeyModifiers::CONTROL),
    (0x0008_0000, KeyModifiers::ALT),
    (0x0010_0000, KeyModifiers::SUPER),
];

/// `kVK_ANSI_*` (HIToolbox Events.h) by keycode; `\0` where the key has a name.
// ponytail: US layout for keys held with ctrl, alt or cmd; a layout lookup if
// someone on another layout needs it.
const US: &[u8; 51] = b"asdfhgzxcv\0bqweryt123465=97-80]ou[ip\0lj'k;\\,/nm.\0\0`";

/// A macOS key-down as a terminal key event: the virtual keycode, the event
/// flags and the text the key typed.
pub fn key_event(code: u16, flags: u64, text: &str) -> Option<KeyEvent> {
    let mods = FLAGS
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .fold(KeyModifiers::NONE, |m, (_, k)| m | *k);
    let named = match code {
        0x24 | 0x4C => KeyCode::Enter,
        0x30 => KeyCode::Tab,
        0x31 => KeyCode::Char(' '),
        0x33 => KeyCode::Backspace,
        0x35 => KeyCode::Esc,
        0x75 => KeyCode::Delete,
        0x73 => KeyCode::Home,
        0x77 => KeyCode::End,
        0x74 => KeyCode::PageUp,
        0x79 => KeyCode::PageDown,
        0x7B => KeyCode::Left,
        0x7C => KeyCode::Right,
        0x7D => KeyCode::Down,
        0x7E => KeyCode::Up,
        0x7A => KeyCode::F(1),
        0x78 => KeyCode::F(2),
        0x63 => KeyCode::F(3),
        0x76 => KeyCode::F(4),
        0x60 => KeyCode::F(5),
        0x61 => KeyCode::F(6),
        0x62 => KeyCode::F(7),
        0x64 => KeyCode::F(8),
        0x65 => KeyCode::F(9),
        0x6D => KeyCode::F(10),
        0x67 => KeyCode::F(11),
        0x6F => KeyCode::F(12),
        _ if mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) => {
            let &c = US.get(code as usize).filter(|&&c| c != 0)?;
            KeyCode::Char(c as char)
        }
        _ => {
            let mut chars = text.chars();
            let c = chars.next().filter(|c| !c.is_control())?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(c)
        }
    };
    Some(KeyEvent::new(named, mods))
}

/// Sends every key typed into Zed until wkey exits. Fails until macOS allows
/// the terminal's app (Zed) to watch the keyboard.
#[cfg(target_os = "macos")]
pub fn watch(tx: std::sync::mpsc::Sender<KeyEvent>) -> anyhow::Result<()> {
    mac::watch(tx)
}

#[cfg(not(target_os = "macos"))]
pub fn watch(_: std::sync::mpsc::Sender<KeyEvent>) -> anyhow::Result<()> {
    anyhow::bail!("keys typed into Zed are only watched on macOS")
}

/// A listen-only CoreGraphics event tap: it sees key presses, never changes them.
#[cfg(target_os = "macos")]
mod mac {
    use super::key_event;
    use anyhow::{Result, bail};
    use ratatui::crossterm::event::KeyEvent;
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::sync::{OnceLock, mpsc};

    type Ref = *mut c_void;
    type Callback = extern "C" fn(Ref, u32, Ref, *mut c_void) -> Ref;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            mask: u64,
            callback: Callback,
            info: *mut c_void,
        ) -> Ref;
        fn CGEventTapEnable(tap: Ref, enable: bool);
        fn CGEventGetIntegerValueField(event: Ref, field: u32) -> i64;
        fn CGEventGetFlags(event: Ref) -> u64;
        fn CGEventKeyboardGetUnicodeString(event: Ref, max: usize, len: *mut usize, buf: *mut u16);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopCommonModes: *const c_void;
        fn CFMachPortCreateRunLoopSource(alloc: *const c_void, port: Ref, order: isize) -> Ref;
        fn CFRunLoopGetCurrent() -> Ref;
        fn CFRunLoopAddSource(run_loop: Ref, source: Ref, mode: *const c_void);
        fn CFRunLoopRun();
    }

    unsafe extern "C" {
        fn proc_pidpath(pid: i32, buf: *mut c_void, size: u32) -> i32;
    }

    // CGEventTypes.h
    const SESSION_TAP: u32 = 1;
    const HEAD_INSERT: u32 = 0;
    const LISTEN_ONLY: u32 = 1;
    const KEY_DOWN: u32 = 10;
    const KEYCODE: u32 = 9;
    const TARGET_PID: u32 = 40;
    const DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
    const DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

    const DENIED: &str = "to follow keys typed in Zed, allow Zed in System Settings > \
                          Privacy & Security > Input Monitoring, then restart Zed";

    static TX: OnceLock<mpsc::Sender<KeyEvent>> = OnceLock::new();
    static PORT: OnceLock<usize> = OnceLock::new();

    pub fn watch(tx: mpsc::Sender<KeyEvent>) -> Result<()> {
        // SAFETY: plain CoreGraphics calls; the second may show macOS's prompt.
        if !unsafe { CGPreflightListenEventAccess() } {
            unsafe { CGRequestListenEventAccess() };
            bail!(DENIED);
        }
        if TX.set(tx).is_err() {
            bail!("keys are already watched");
        }
        let (ready, started) = mpsc::channel();
        std::thread::spawn(move || {
            // SAFETY: the tap and its run loop source live as long as the process.
            unsafe {
                let port = CGEventTapCreate(
                    SESSION_TAP,
                    HEAD_INSERT,
                    LISTEN_ONLY,
                    1 << KEY_DOWN,
                    on_event,
                    null_mut(),
                );
                if port.is_null() {
                    let _ = ready.send(false);
                    return;
                }
                let _ = PORT.set(port as usize);
                let source = CFMachPortCreateRunLoopSource(null(), port, 0);
                CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
                let _ = ready.send(true);
                CFRunLoopRun();
            }
        });
        if started.recv()? {
            Ok(())
        } else {
            bail!(DENIED)
        }
    }

    extern "C" fn on_event(_: Ref, kind: u32, event: Ref, _: *mut c_void) -> Ref {
        // SAFETY: `event` is the live event macOS hands the callback.
        unsafe {
            if matches!(kind, DISABLED_BY_TIMEOUT | DISABLED_BY_USER_INPUT) {
                if let Some(&port) = PORT.get() {
                    CGEventTapEnable(port as Ref, true);
                }
                return event;
            }
            if kind != KEY_DOWN || !is_zed(CGEventGetIntegerValueField(event, TARGET_PID) as i32) {
                return event;
            }
            let code = CGEventGetIntegerValueField(event, KEYCODE) as u16;
            let mut buf = [0u16; 4];
            let mut len = 0;
            CGEventKeyboardGetUnicodeString(event, buf.len(), &mut len, buf.as_mut_ptr());
            let text = String::from_utf16_lossy(&buf[..len.min(buf.len())]);
            let flags = CGEventGetFlags(event);
            let ev = key_event(code, flags, &text);
            crate::log::event(format_args!(
                "tap  code={code:#04x} flags={flags:#x} text={text:?} -> {}",
                ev.and_then(crate::tui::zed_key)
                    .as_deref()
                    .unwrap_or("skipped")
            ));
            if let (Some(ev), Some(tx)) = (ev, TX.get()) {
                let _ = tx.send(ev);
            }
        }
        event
    }

    thread_local! {
        static LAST: Cell<(i32, bool)> = const { Cell::new((0, false)) };
    }

    /// Whether the key goes to Zed (`Zed.app/Contents/MacOS/zed`, a preview or a dev build).
    fn is_zed(pid: i32) -> bool {
        LAST.with(|last| {
            let (seen, zed) = last.get();
            if seen == pid && pid != 0 {
                return zed;
            }
            let mut buf = [0u8; 4096];
            // SAFETY: the buffer is PROC_PIDPATHINFO_MAXSIZE bytes.
            let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
            let zed = n > 0 && buf[..n as usize].ends_with(b"/zed");
            last.set((pid, zed));
            zed
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::zed_key;

    const SHIFT: u64 = 0x0002_0000;
    const CONTROL: u64 = 0x0004_0000;
    const ALT: u64 = 0x0008_0000;
    const CMD: u64 = 0x0010_0000;

    fn zed(code: u16, flags: u64, text: &str) -> Option<String> {
        key_event(code, flags, text).and_then(zed_key)
    }

    #[test]
    fn typed_text_names_plain_and_shifted_keys() {
        assert_eq!(zed(0x02, 0, "d").as_deref(), Some("d"));
        assert_eq!(zed(0x05, SHIFT, "G").as_deref(), Some("shift-g"));
        assert_eq!(zed(0x15, SHIFT, "$").as_deref(), Some("$"));
        assert_eq!(zed(0x31, 0, " ").as_deref(), Some("space"));
    }

    #[test]
    fn modified_keys_use_the_key_not_the_typed_text() {
        assert_eq!(zed(0x0D, CONTROL, "\u{17}").as_deref(), Some("ctrl-w"));
        assert_eq!(zed(0x02, ALT, "∂").as_deref(), Some("alt-d"));
        assert_eq!(zed(0x01, CMD, "s").as_deref(), Some("cmd-s"));
        assert_eq!(
            zed(0x0D, CONTROL | SHIFT, "\u{17}").as_deref(),
            Some("ctrl-shift-w")
        );
    }

    #[test]
    fn named_keys_come_from_the_keycode() {
        assert_eq!(zed(0x35, 0, "\u{1b}").as_deref(), Some("escape"));
        assert_eq!(zed(0x24, 0, "\r").as_deref(), Some("enter"));
        assert_eq!(zed(0x30, SHIFT, "\t").as_deref(), Some("shift-tab"));
        assert_eq!(zed(0x33, 0, "\u{7f}").as_deref(), Some("backspace"));
        assert_eq!(zed(0x7B, 0, "").as_deref(), Some("left"));
        // Keys with no name and no text, such as volume keys, are skipped.
        assert_eq!(zed(0x48, 0, ""), None);
    }
}
