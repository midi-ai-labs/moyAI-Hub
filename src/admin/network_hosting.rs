//! The last successfully started endpoint, separate from device identity and rules.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::Write,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    schema_version: u32,
    pub bind: SocketAddr,
    pub advertise_ip: IpAddr,
}

impl Settings {
    pub fn new(bind: &str, advertise_ip: &str) -> Result<Self, String> {
        let settings = Self {
            schema_version: 1,
            bind: bind
                .parse()
                .map_err(|_| "待受先を IP:port で指定してください。")?,
            advertise_ip: advertise_ip
                .parse()
                .map_err(|_| "接続先へ案内する具体的な IP アドレスを指定してください。")?,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("ネットワーク起動設定の形式に対応していません。".into());
        }
        if self.bind.port() == 0
            || self.advertise_ip.is_unspecified()
            || self.advertise_ip.is_multicast()
            || (!self.bind.ip().is_unspecified() && self.bind.ip() != self.advertise_ip)
        {
            return Err("待受先と案内する IP アドレスを確認してください。".into());
        }
        Ok(())
    }
}

pub(super) struct Store {
    path: PathBuf,
    loaded: Result<Option<Settings>, String>,
}

impl Store {
    pub fn open(path: PathBuf) -> Self {
        let loaded = (|| {
            let metadata = match std::fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(_) => return Err("ネットワーク起動設定を読み込めません。".into()),
            };
            if !metadata.is_file() || metadata.len() > 16 * 1024 {
                return Err("ネットワーク起動設定のファイルとサイズを確認してください。".into());
            }
            let bytes =
                std::fs::read(&path).map_err(|_| "ネットワーク起動設定を読み込めません。")?;
            let settings: Settings = serde_json::from_slice(&bytes)
                .map_err(|_| "ネットワーク起動設定の形式を確認してください。")?;
            settings.validate()?;
            Ok(Some(settings))
        })();
        Self { path, loaded }
    }

    pub fn configured(&self) -> Result<Settings, String> {
        self.loaded.clone()?.ok_or_else(|| {
            "ネットワーク起動設定がありません。管理画面でこのHubのIPとポートを指定し、ネットワークを開始してください。".into()
        })
    }

    pub fn snapshot(&self) -> Value {
        match &self.loaded {
            Ok(Some(settings)) => json!({"configured":true,"bind":settings.bind,
                "advertise_ip":settings.advertise_ip,"error":null}),
            Ok(None) => json!({"configured":false,"bind":null,"advertise_ip":null,"error":null}),
            Err(error) => json!({"configured":false,"bind":null,"advertise_ip":null,"error":error}),
        }
    }

    pub fn save(&mut self, settings: Settings) -> Result<(), String> {
        settings.validate()?;
        let parent = self.path.parent().ok_or("起動設定の保存先がありません。")?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| "ネットワーク起動設定の一時ファイルを作成できません。")?;
        serde_json::to_writer_pretty(&mut temp, &settings)
            .map_err(|_| "ネットワーク起動設定を書き込めません。")?;
        temp.write_all(b"\n")
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|_| "ネットワーク起動設定をディスクに同期できません。")?;
        temp.persist(&self.path)
            .map_err(|_| "ネットワーク起動設定を保存できません。元の設定は保持されています。")?;
        self.loaded = Ok(Some(settings));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_settings_survive_reopen_without_guessing_missing_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("network-hosting.json");
        let mut owner = Store::open(path.clone());
        assert_eq!(
            owner.snapshot(),
            json!({"configured":false,"bind":null,"advertise_ip":null,"error":null})
        );
        assert!(owner.configured().unwrap_err().contains("管理画面"));
        assert!(!path.exists());
        let settings = Settings::new("0.0.0.0:9471", "192.168.1.10").unwrap();
        owner.save(settings.clone()).unwrap();
        assert_eq!(Store::open(path.clone()).configured().unwrap(), settings);
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),
            json!({"schema_version":1,"bind":"0.0.0.0:9471","advertise_ip":"192.168.1.10"})
        );
        let ipv6 = Settings::new("[::1]:9471", "::1").unwrap();
        owner.save(ipv6.clone()).unwrap();
        assert_eq!(Store::open(owner.path.clone()).configured().unwrap(), ipv6);
    }

    #[test]
    fn corrupt_and_unsupported_settings_remain_untouched_until_explicit_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("network-hosting.json");
        for bytes in [
            b"not JSON".as_slice(),
            br#"{"schema_version":2,"bind":"127.0.0.1:9471","advertise_ip":"127.0.0.1"}"#,
            br#"{"schema_version":1,"bind":"127.0.0.1:0","advertise_ip":"127.0.0.1"}"#,
            br#"{"schema_version":1,"bind":"127.0.0.1:9471","advertise_ip":"192.168.1.10"}"#,
        ] {
            std::fs::write(&path, bytes).unwrap();
            let mut owner = Store::open(path.clone());
            assert!(owner.configured().is_err());
            assert_eq!(owner.snapshot()["configured"], false);
            assert!(owner.snapshot()["error"].is_string());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            owner
                .save(Settings::new("127.0.0.1:9471", "127.0.0.1").unwrap())
                .unwrap();
            assert!(Store::open(path.clone()).configured().is_ok());
            assert!(owner.snapshot()["error"].is_null());
        }
    }

    #[test]
    fn failed_replacement_preserves_the_loaded_setting_and_removes_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("network-hosting.json");
        let mut owner = Store::open(path.clone());
        let settings = Settings::new("127.0.0.1:9471", "127.0.0.1").unwrap();
        owner.save(settings.clone()).unwrap();
        // Keep the old bytes at a separate path while a directory blocks replacement.
        let original = directory.path().join("original.json");
        std::fs::rename(&path, &original).unwrap();
        std::fs::create_dir(&path).unwrap();
        let bytes = std::fs::read(&original).unwrap();
        assert!(
            owner
                .save(Settings::new("127.0.0.1:9472", "127.0.0.1").unwrap())
                .is_err()
        );
        assert_eq!(owner.configured().unwrap(), settings);
        assert_eq!(std::fs::read(original).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn endpoint_validation_is_shared_by_explicit_start_and_saved_start() {
        for (bind, public) in [
            ("localhost:9471", "127.0.0.1"),
            ("127.0.0.1:0", "127.0.0.1"),
            ("0.0.0.0:9471", "0.0.0.0"),
            ("0.0.0.0:9471", "224.0.0.1"),
            ("127.0.0.1:9471", "192.168.1.10"),
            ("127.0.0.1:9471", "localhost"),
        ] {
            assert!(Settings::new(bind, public).is_err(), "{bind}, {public}");
        }
    }
}
