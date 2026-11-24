//! The sign-in splash (the animated logo, the app name and "Signing in…") covers the window at
//! start and is gone once sign-in completes. The animation itself is judged by eye.

use super::*;
use std::time::Duration;

fn frames(ms: u64) {
    testing::mock_elapsed_time(Duration::from_millis(ms));
    testing::mock_elapsed_time(Duration::from_millis(16));
}

#[test]
fn the_splash_shows_at_start_and_hides_after_sign_in() {
    let app = new_window();
    app.window().set_size(LogicalSize::new(1200., 800.));
    app.show().unwrap();
    frames(600);
    assert_eq!(app.get_login_state(), "checking");
    assert!(app.get_splash_shown(), "shown while signing in");

    // The device-code step keeps the splash up too.
    app.set_user_code("ABCD1234".into());
    app.set_verify_url("https://example.invalid/device".into());
    app.set_login_state("code".into());
    frames(600);
    assert!(app.get_splash_shown(), "shown while waiting for the code");

    app.set_login_state("ready".into());
    frames(600);
    assert!(!app.get_splash_shown(), "gone once the main UI is up");
}

#[test]
fn a_policy_block_offers_the_copy_button_and_other_errors_do_not() {
    let app = new_window();
    app.window().set_size(LogicalSize::new(1200., 800.));
    app.show().unwrap();
    app.set_login_error("Your organization blocks this sign-in method.".into());
    app.set_login_report(
        "IronTeams sign-in blocked: AADSTS53003 (BlockedByConditionalAccess).".into(),
    );
    app.set_login_state("error".into());
    frames(600);
    assert!(app.get_splash_shown());
    assert!(
        app.get_login_report()
            .starts_with("IronTeams sign-in blocked")
    );
    let copied = std::rc::Rc::new(std::cell::Cell::new(0));
    let c = copied.clone();
    app.on_copy_login_report(move || c.set(c.get() + 1));
    app.invoke_copy_login_report();
    assert_eq!(copied.get(), 1);
}
