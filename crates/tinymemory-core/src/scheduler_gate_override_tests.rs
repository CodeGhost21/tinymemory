use super::{clear_manual_override, current_policy, set_manual_override, Policy};

#[test]
fn a_zero_second_window_is_already_expired_and_the_clamp_holds() {
    clear_manual_override();
    // Zero seconds: the window closes the instant it opens — the branch
    // in `current_policy` must treat an expired window as no window.
    set_manual_override(0);
    assert_eq!(current_policy(), Policy::Normal); // ungated baseline
                                                  // An absurd ask cannot overflow the expiry arithmetic: the clamp is
                                                  // the function's contract, and this call not panicking is the test.
    set_manual_override(u64::MAX);
    assert_eq!(current_policy(), Policy::Normal);
    clear_manual_override();
}
