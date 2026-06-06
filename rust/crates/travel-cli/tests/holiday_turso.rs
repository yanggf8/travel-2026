#[tokio::test]
async fn leave_calc_can_read_holidays_from_turso_when_credentials_exist() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_travel"))
        .args(["leave", "calc", "2026-06-20", "2026-06-24", "taiwan"])
        .output()
        .expect("run travel leave calc");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("turso auth login")
            || stderr.contains("Missing Turso data")
            || stderr.contains("failed to connect to Turso")
        {
            eprintln!("skipping Turso-backed leave-calc test: {}", stderr.trim());
            return;
        }
        panic!("travel leave calc failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Trip: 2026-06-20 to 2026-06-24 (5 days)"));
    assert!(stdout.contains("Leave days required:"));
}
