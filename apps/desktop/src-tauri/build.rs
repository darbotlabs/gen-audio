// The fixture commands (and the capability that allows them) exist only when
// the app is compiled with debug_assertions, mirroring `invoke_handler` in
// src/lib.rs (E1 addendum). `tauri build` compiles release, so its exe carries
// neither the commands nor their permissions.
const RELEASE_COMMANDS: &[&str] = &["connector_statuses", "mcp_status"];
const DEBUG_COMMANDS: &[&str] = &[
    "connector_statuses",
    "viewport_example",
    "run_fixture_improve",
    "mcp_status",
];

fn main() {
    // Release embeds only capabilities/default.json. An empty capabilities
    // list would embed every file in capabilities/, including dev.json and
    // dev-fixtures.json. tauri.conf.json lists only "default". Debug builds
    // merge the fixture capabilities; the handlers stay cfg(debug_assertions).
    let debug = std::env::var_os("CARGO_CFG_DEBUG_ASSERTIONS").is_some();
    let commands = if debug {
        DEBUG_COMMANDS
    } else {
        RELEASE_COMMANDS
    };
    let mut attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(commands));
    if debug {
        std::env::set_var(
            "TAURI_CONFIG",
            r#"{"app":{"security":{"capabilities":["default","dev","dev-fixtures"]}}}"#,
        );
    } else {
        attributes = attributes.capabilities_path_pattern("./capabilities/default.json");
    }
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
