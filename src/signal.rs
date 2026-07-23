use std::sync::atomic::{AtomicBool, Ordering};

static SIGWINCH_RECEIVED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
extern "C" fn sigwinch_handler(_: core::ffi::c_int) {
    SIGWINCH_RECEIVED.store(true, Ordering::Relaxed);
}

#[cfg(target_os = "linux")]
unsafe extern "C" fn sigwinch_handler(_: core::ffi::c_int) {
    SIGWINCH_RECEIVED.store(true, Ordering::Relaxed);
}

pub fn register_sigwinch() {
    #[cfg(target_os = "linux")]
    {
        use rustix::runtime::{
            KernelSigSet, KernelSigaction, KernelSigactionFlags, kernel_sigaction,
        };
        use rustix::signal::Signal;

        // SAFETY: The handler only writes to an `AtomicBool`, which is async-signal-safe.
        // The action uses an empty signal mask and requests SA_RESTART, matching the previous
        // libc implementation. `kernel_sigaction` is unsafe because signal handlers are
        // inherently unsafe and its caller must uphold the platform ABI requirements.
        unsafe {
            let action = KernelSigaction {
                sa_handler_kernel: Some(sigwinch_handler),
                sa_flags: KernelSigactionFlags::RESTART,
                sa_restorer: None,
                sa_mask: KernelSigSet::empty(),
            };
            let _ = kernel_sigaction(Signal::WINCH, Some(action));
        }
    }

    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct MacSigaction {
            sa_sigaction: usize,
            sa_mask: u32,
            sa_flags: core::ffi::c_int,
        }

        unsafe extern "C" {
            fn sigaction(
                signal: core::ffi::c_int,
                action: *const MacSigaction,
                old_action: *mut MacSigaction,
            ) -> core::ffi::c_int;
        }

        // SAFETY: The Darwin `sigaction` ABI uses a function pointer, 32-bit signal mask, and
        // integer flags in this order. The handler is a valid C ABI function, the mask is empty,
        // and the old action is not needed. These definitions are kept local because rustix's
        // signal-installation runtime API is Linux-only and libc is intentionally not a
        // dependency of this crate.
        unsafe {
            let action = MacSigaction {
                sa_sigaction: sigwinch_handler as *const () as usize,
                sa_mask: 0,
                sa_flags: 0x0002,
            };
            let _ = sigaction(28, &action, core::ptr::null_mut());
        }
    }
}

pub fn take_sigwinch() -> bool {
    SIGWINCH_RECEIVED.swap(false, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_take() {
        register_sigwinch();
        // Initially false (or may have been set by previous test — clear it)
        let _ = take_sigwinch();
        assert!(!take_sigwinch());
    }

    #[test]
    fn test_signal_delivery() {
        register_sigwinch();
        let _ = take_sigwinch(); // clear
        // Directly set the flag to test take_sigwinch
        SIGWINCH_RECEIVED.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(take_sigwinch());
        assert!(!take_sigwinch()); // consumed
    }
}
