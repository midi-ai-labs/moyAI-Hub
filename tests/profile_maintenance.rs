//! Exercise interrupted default moves in child processes with isolated Windows profile locations.
#[cfg(windows)]
mod windows {
    use moyai_hub::{admin::Admin, data_paths, maintenance};
    use std::{fs, path::Path, process::Command};

    fn fixture_parent() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("project_sandbox/hub-profile-process-tests")
    }

    #[test]
    fn explicit_paths_and_library_owners_cannot_open_an_interrupted_default_move() {
        let parent = fixture_parent();
        fs::create_dir_all(&parent).unwrap();
        let temp = tempfile::tempdir_in(&parent).unwrap();
        let local = temp.path().join("local");
        let roaming = temp.path().join("roaming");
        let paths = data_paths::windows_paths(&local, &roaming);
        fs::create_dir_all(&paths.current).unwrap();
        fs::create_dir_all(&paths.legacy).unwrap();
        fs::create_dir_all(paths.marker.parent().unwrap()).unwrap();
        fs::write(&paths.marker, br#"{"schema":1,"state":"moving"}"#).unwrap();
        fs::write(
            paths.current.join("shared-work.sqlite3"),
            b"moved database fixture",
        )
        .unwrap();
        fs::write(
            paths.legacy.join("device-network.json"),
            b"unmoved credential fixture",
        )
        .unwrap();
        for directory in [&paths.current, &paths.legacy] {
            let commands: Vec<Vec<std::ffi::OsString>> = vec![
                vec!["--data-dir".into(), directory.as_os_str().to_os_string()],
                vec![
                    "--launch".into(),
                    "--no-browser".into(),
                    "--data-dir".into(),
                    directory.as_os_str().to_os_string(),
                ],
                vec![
                    "--backup-dir".into(),
                    temp.path().join("backup").into(),
                    "--data-dir".into(),
                    directory.as_os_str().to_os_string(),
                ],
                vec![
                    "--restore-from".into(),
                    temp.path().join("backup").into(),
                    "--acknowledge-runners-stopped".into(),
                    "--data-dir".into(),
                    directory.as_os_str().to_os_string(),
                ],
            ];
            for arguments in commands {
                let output = Command::new(env!("CARGO_BIN_EXE_moyai-hub"))
                    .env("LOCALAPPDATA", &local)
                    .env("APPDATA", &roaming)
                    .args(arguments)
                    .output()
                    .unwrap();
                assert!(!output.status.success());
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("移行記録"),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let output = Command::new(env!("CARGO_BIN_EXE_moyai-cleanup-hub"))
                .env("LOCALAPPDATA", &local)
                .env("APPDATA", &roaming)
                .args(["--reset-user-data", "--confirm-delete", "--data-dir"])
                .arg(directory)
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("移行記録"));
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .env("LOCALAPPDATA", &local)
            .env("APPDATA", &roaming)
            .args([
                "--exact",
                "windows::direct_library_owners_refuse_pending_profiles",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("pending owners verified"));
        assert_eq!(
            fs::read(paths.current.join("shared-work.sqlite3")).unwrap(),
            b"moved database fixture"
        );
        assert_eq!(
            fs::read(paths.legacy.join("device-network.json")).unwrap(),
            b"unmoved credential fixture"
        );
        assert!(!paths.current.join("catalog.json").exists());
        assert!(!paths.current.join("device-network.json").exists());
        assert!(!paths.legacy.join("catalog.json").exists());
    }

    #[test]
    fn direct_library_owners_refuse_pending_profiles() {
        let Ok(paths) = data_paths::default_paths() else {
            return;
        };
        // Only the parent test's isolated process activates this helper; never inspect user data.
        if !paths.current.starts_with(fixture_parent()) {
            return;
        }
        let root = paths
            .current
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        for directory in [&paths.current, &paths.legacy] {
            assert!(
                Admin::open(directory.join("catalog.json"))
                    .err()
                    .unwrap()
                    .contains("移行記録")
            );
            assert!(
                moyai_hub::backup::backup(directory, &root.join("blocked-backup"))
                    .err()
                    .unwrap()
                    .contains("移行記録")
            );
            assert!(
                moyai_hub::backup::restore(&root.join("backup"), directory, true)
                    .err()
                    .unwrap()
                    .contains("移行記録")
            );
            assert!(
                maintenance::reset(directory, Some(&paths))
                    .err()
                    .unwrap()
                    .contains("移行記録")
            );
        }
        assert!(!root.join("blocked-backup").exists());
        let custom = root.join("independent-custom");
        let admin = Admin::open(custom.join("catalog.json")).unwrap();
        drop(admin);
        assert!(custom.join("device-network.json").exists());
        println!("pending owners verified");
    }
}
