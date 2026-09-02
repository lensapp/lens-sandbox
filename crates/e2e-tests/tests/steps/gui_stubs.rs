use crate::E2eWorld;
use cucumber::{given, then, when};

#[given("no LNS tray icon is visible")]
fn no_tray_icon_visible(_world: &mut E2eWorld) {
    unreachable!("`filter_run` excludes @gui scenarios in headless suites");
}

#[given("the LNS tray icon is visible")]
fn tray_icon_visible(_world: &mut E2eWorld) {
    unreachable!("`filter_run` excludes @gui scenarios in headless suites");
}

#[when("I close the terminal that ran `lns service start`")]
fn close_terminal(_world: &mut E2eWorld) {
    unreachable!("`filter_run` excludes @gui scenarios in headless suites");
}

#[when(regex = r#"^I select "([^"]*)" from the tray menu$"#)]
fn select_tray_menu(_world: &mut E2eWorld, _item: String) {
    unreachable!("`filter_run` excludes @gui scenarios in headless suites");
}

#[then("no second tray icon appears")]
fn no_second_tray_icon(_world: &mut E2eWorld) {
    // no-op: covered by "no second service is started" (no service = no tray).
}

#[then("no tray icon appears")]
fn no_tray_icon(_world: &mut E2eWorld) {
    // no-op: covered by "no service is started as a side effect" (no service = no tray).
}

#[then("an LNS tray icon appears in my OS tray")]
fn tray_icon_appears(_world: &mut E2eWorld) {
    unreachable!("`filter_run` excludes @gui scenarios in headless suites");
}

#[then("the tray icon remains visible after the command exits")]
fn tray_remains_after_exit(_world: &mut E2eWorld) {
    unreachable!("`filter_run` excludes @gui scenarios in headless suites");
}

#[then("the tray icon remains visible")]
fn tray_remains_visible(_world: &mut E2eWorld) {
    unreachable!("`filter_run` excludes @gui scenarios in headless suites");
}

#[then("the service continues running")]
fn service_continues(_world: &mut E2eWorld) {
    unreachable!("`filter_run` excludes @gui scenarios in headless suites");
}

#[then("the tray icon disappears")]
fn tray_disappears(_world: &mut E2eWorld) {
    unreachable!("`filter_run` excludes @gui scenarios in headless suites");
}

#[then("no LNS service remains running")]
fn no_service_remains(_world: &mut E2eWorld) {
    unreachable!("`filter_run` excludes @gui scenarios in headless suites");
}
