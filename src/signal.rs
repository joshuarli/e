use std::sync::atomic::{AtomicBool, Ordering};

static SIGWINCH_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigwinch_handler(_: libc::c_int) {
    SIGWINCH_RECEIVED.store(true, Ordering::Relaxed);
}

pub fn register_sigwinch() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = sigwinch_handler as *const () as usize;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGWINCH, &sa, std::ptr::null_mut());
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
