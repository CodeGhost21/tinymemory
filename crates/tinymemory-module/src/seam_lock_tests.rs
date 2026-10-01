//! Tests for the global-seam test lock.
//!
//! One test, and it is about the lock rather than about any seam: everything
//! [`super`] claims rests on the lock actually being exclusive.

/// The lock is actually exclusive.
///
/// Worth pinning, because everything in [`super`] only helps if this holds, and a
/// refactor could quietly make it a no-op — a second `OnceLock`, a guard
/// dropped at the end of its own statement rather than the test body — without
/// any test noticing. The failure it guards against is invisible by nature:
/// the suite would simply go back to being flaky somewhere else.
#[test]
fn only_one_thread_holds_the_seams_at_a_time() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static INSIDE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..200 {
                    let _seams = super::hold_global_seams();
                    let now = INSIDE.fetch_add(1, Ordering::SeqCst) + 1;
                    PEAK.fetch_max(now, Ordering::SeqCst);
                    std::thread::yield_now();
                    INSIDE.fetch_sub(1, Ordering::SeqCst);
                }
            });
        }
    });

    assert_eq!(
        PEAK.load(Ordering::SeqCst),
        1,
        "two threads held the global seams at once, so serialising the tests \
         that install them buys nothing"
    );
}
