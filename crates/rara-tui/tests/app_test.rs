//! Tests for the TUI application state logic.

use std::path::PathBuf;

use rara_tui::app::{App, TAB_NAMES};

/// Create an app with a non-existent promoted dir (yields empty strategy list).
fn test_app() -> App {
    App::new(
        "http://localhost:50051".to_owned(),
        PathBuf::from("/tmp/rara-tui-test-nonexistent"),
    )
}

#[test]
fn app_tab_navigation_clamps_to_valid_range() {
    let mut app = test_app();
    assert_eq!(app.active_tab, 0);

    // Navigate to last valid tab
    app.select_tab(TAB_NAMES.len() - 1);
    assert_eq!(app.active_tab, TAB_NAMES.len() - 1);

    // Out-of-range index is silently ignored (tab stays unchanged)
    app.select_tab(TAB_NAMES.len());
    assert_eq!(
        app.active_tab,
        TAB_NAMES.len() - 1,
        "out-of-range index should be ignored"
    );

    app.select_tab(999);
    assert_eq!(
        app.active_tab,
        TAB_NAMES.len() - 1,
        "wildly out-of-range index should be ignored"
    );

    // Navigate back to first tab
    app.select_tab(0);
    assert_eq!(app.active_tab, 0);
}

#[test]
fn app_quit_sets_running_false() {
    let mut app = test_app();
    assert!(app.running, "app should start in running state");

    app.quit();
    assert!(!app.running, "quit() should set running to false");
}
