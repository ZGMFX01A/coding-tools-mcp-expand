use std::sync::Mutex;

use crate::error::{AppError, AppResult};
use crate::settings::AppSettings;
use crate::workspace::legacy_import::import_legacy_profiles_if_empty;
use crate::workspace::WorkspaceProfile;

use super::migrate::{data_file_path, load_or_migrate, maybe_backup_legacy_files, save};
use super::model::AppData;

static DATA_FILE_LOCK: Mutex<()> = Mutex::new(());

const SHARED_KEYS: &[&str] = &[
    "oauth_client_id",
    "bearer_token",
    "oauth_client_secret",
    "oauth_password",
    "oauth_token_secret",
    "actions_api_key",
    "actions_oauth_client_secret",
    "actions_oauth_password",
    "actions_oauth_token_secret",
];

#[derive(Debug)]
pub struct DataStore {
    data: AppData,
}

impl DataStore {
    pub fn load() -> AppResult<Self> {
        let _guard = lock_data_file()?;
        let path = data_file_path()?;
        let existed_before = path.exists();
        let mut data = load_or_migrate()?;
        let imported = import_legacy_profiles_if_empty(&mut data)?;
        let store = Self { data };
        if !existed_before || imported > 0 {
            store.persist_unlocked()?;
        }
        if !existed_before {
            maybe_backup_legacy_files(&path)?;
        }
        Ok(store)
    }

    pub fn read_file<R>(f: impl FnOnce(&AppData) -> AppResult<R>) -> AppResult<R> {
        let _guard = lock_data_file()?;
        let data = load_or_migrate()?;
        f(&data)
    }

    pub fn update_file<R>(f: impl FnOnce(&mut AppData) -> AppResult<R>) -> AppResult<R> {
        let _guard = lock_data_file()?;
        let mut data = load_or_migrate()?;
        let result = f(&mut data)?;
        save(&data)?;
        Ok(result)
    }

    pub fn data(&self) -> &AppData {
        &self.data
    }

    pub fn save(&self) -> AppResult<()> {
        let _guard = lock_data_file()?;
        self.persist_unlocked()
    }

    fn persist_unlocked(&self) -> AppResult<()> {
        save(&self.data)
    }

    pub fn settings(&self) -> AppSettings {
        AppSettings::from_data(&self.data)
    }

    pub fn update_settings(&mut self, settings: AppSettings) -> AppResult<()> {
        settings.apply_to(&mut self.data);
        self.save()
    }

    pub fn list(&self) -> &[WorkspaceProfile] {
        &self.data.profiles
    }

    pub fn get(&self, id: &str) -> Option<&WorkspaceProfile> {
        self.data.profiles.iter().find(|profile| profile.id == id)
    }

    pub fn add(&mut self, profile: WorkspaceProfile) -> AppResult<()> {
        self.data.profiles.push(profile);
        self.save()
    }

    pub fn update(&mut self, profile: WorkspaceProfile) -> AppResult<()> {
        let Some(index) = self
            .data
            .profiles
            .iter()
            .position(|item| item.id == profile.id)
        else {
            return Err(AppError::Message(format!(
                "workspace not found: {}",
                profile.id
            )));
        };
        self.data.profiles[index] = profile;
        self.save()
    }

    /// 修改工作区 ID（主键重命名）：
    /// 迁移该工作区的 secrets、更新 last_workspace_id 引用，并保存。
    /// 调用方须先完成运行时停止与 ID 校验；这里只做迁移和唯一性兜底。
    pub fn rename_workspace_id(&mut self, old_id: &str, new_id: &str) -> AppResult<WorkspaceProfile> {
        let index = self
            .data
            .profiles
            .iter()
            .position(|item| item.id == old_id)
            .ok_or_else(|| AppError::Message(format!("workspace not found: {old_id}")))?;

        // 幂等：新旧 ID 相同视为无操作，直接返回当前 profile
        if old_id == new_id {
            return Ok(self.data.profiles[index].clone());
        }

        // 唯一性兜底：新 ID 不能与任何已有 profile 冲突
        if self.data.profiles.iter().any(|item| item.id == new_id) {
            return Err(AppError::Message(format!(
                "workspace id already exists: {new_id}"
            )));
        }

        // 迁移工作区 secrets（以 profile_id 为 key）
        if let Some(secrets) = self.data.workspace_secrets.remove(old_id) {
            self.data.workspace_secrets.insert(new_id.to_string(), secrets);
        }

        // 更新 profile 主键
        self.data.profiles[index].id = new_id.to_string();
        let profile = self.data.profiles[index].clone();

        // 若记住的最近工作区指向旧 ID，同步改为新 ID
        if self.data.last_workspace_id == old_id {
            self.data.last_workspace_id = new_id.to_string();
        }

        self.save()?;
        Ok(profile)
    }

    pub fn remove(&mut self, id: &str) -> AppResult<Option<WorkspaceProfile>> {
        let Some(index) = self.data.profiles.iter().position(|item| item.id == id) else {
            return Ok(None);
        };
        let removed = self.data.profiles.remove(index);
        self.data.workspace_secrets.remove(id);
        self.save()?;
        Ok(Some(removed))
    }

    pub fn init_workspace_secrets(&mut self, profile_id: &str) -> AppResult<()> {
        // oauth_client_secret is optional for MCP OAuth (ChatGPT PKCE); not auto-generated.
        self.set_workspace_secret(profile_id, "oauth_password", &random_secret())?;
        self.set_workspace_secret(profile_id, "oauth_token_secret", &random_secret())?;
        self.set_workspace_secret(profile_id, "bearer_token", &random_secret())?;
        self.set_workspace_secret(profile_id, "actions_api_key", &random_secret())?;
        self.set_workspace_secret(profile_id, "actions_oauth_client_secret", &random_secret())?;
        self.set_workspace_secret(profile_id, "actions_oauth_password", &random_secret())?;
        self.set_workspace_secret(profile_id, "actions_oauth_token_secret", &random_secret())?;
        Ok(())
    }

    pub fn init_shared_secrets(&mut self) -> AppResult<()> {
        let mut changed = false;
        for key in SHARED_KEYS {
            if !self.data.shared_secrets.contains_key(*key) {
                self.data
                    .shared_secrets
                    .insert(key.to_string(), shared_value_for_key(key));
                changed = true;
            }
        }
        if changed {
            self.save()?;
        }
        Ok(())
    }

    pub fn get_workspace_secret(&self, profile_id: &str, key: &str) -> AppResult<Option<String>> {
        Ok(self
            .data
            .workspace_secrets
            .get(profile_id)
            .and_then(|secrets| secrets.get(key))
            .filter(|value| !value.is_empty())
            .cloned())
    }

    pub fn set_workspace_secret(
        &mut self,
        profile_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()> {
        self.data
            .workspace_secrets
            .entry(profile_id.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
        self.save()
    }

    pub fn regenerate_workspace_secret(&mut self, profile_id: &str, key: &str) -> AppResult<String> {
        let value = shared_value_for_key(key);
        self.set_workspace_secret(profile_id, key, &value)?;
        Ok(value)
    }

    pub fn remove_workspace_secrets(&mut self, profile_id: &str) -> AppResult<()> {
        self.data.workspace_secrets.remove(profile_id);
        self.save()
    }

    pub fn get_shared_secret(&self, key: &str) -> Option<String> {
        self.data.shared_secrets.get(key).cloned()
    }

    pub fn set_shared_secret(&mut self, key: &str, value: &str) -> AppResult<()> {
        self.data
            .shared_secrets
            .insert(key.to_string(), value.to_string());
        self.save()
    }

    pub fn regenerate_shared_secret(&mut self, key: &str) -> AppResult<String> {
        let value = random_secret();
        self.set_shared_secret(key, &value)?;
        Ok(value)
    }

    pub fn get_app_secret(&self, scope: &str, item_id: &str) -> Option<String> {
        self.data
            .app_secrets
            .get(scope)
            .and_then(|items| items.get(item_id))
            .filter(|value| !value.is_empty())
            .cloned()
    }

    pub fn set_app_secret(&mut self, scope: &str, item_id: &str, value: &str) -> AppResult<()> {
        self.data
            .app_secrets
            .entry(scope.to_string())
            .or_default()
            .insert(item_id.to_string(), value.to_string());
        self.save()
    }

    pub fn delete_app_secret(&mut self, scope: &str, item_id: &str) -> AppResult<()> {
        if let Some(items) = self.data.app_secrets.get_mut(scope) {
            items.remove(item_id);
            if items.is_empty() {
                self.data.app_secrets.remove(scope);
            }
        }
        self.save()
    }

}

fn lock_data_file() -> AppResult<std::sync::MutexGuard<'static, ()>> {
    DATA_FILE_LOCK
        .lock()
        .map_err(|_| AppError::Message("data file lock poisoned".into()))
}

fn random_secret() -> String {
    format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()).replace('-', "")
}

fn shared_value_for_key(key: &str) -> String {
    if key == "oauth_client_id" {
        format!("chatgpt-client-{}", &uuid::Uuid::new_v4().to_string()[..12])
    } else {
        random_secret()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_secret_roundtrip() {
        let id = uuid::Uuid::new_v4().to_string().replace('-', "");
        let mut store = DataStore::load().expect("load");
        store
            .set_workspace_secret(&id, "oauth_client_secret", "roundtrip-secret")
            .expect("set");
        let loaded = store
            .get_workspace_secret(&id, "oauth_client_secret")
            .expect("get");
        assert_eq!(loaded.as_deref(), Some("roundtrip-secret"));
        store.remove_workspace_secrets(&id).expect("remove");
    }

    #[test]
    fn shared_oauth_client_id_uses_client_id_format() {
        let value = shared_value_for_key("oauth_client_id");
        assert!(value.starts_with("chatgpt-client-"));
        assert_eq!(value.len(), "chatgpt-client-".len() + 12);
    }

    fn temp_profile(id: &str) -> WorkspaceProfile {
        let mut profile = WorkspaceProfile::new("/tmp/workspace-id-edit".into(), None);
        profile.id = id.to_string();
        profile
    }

    #[test]
    fn rename_workspace_id_migrates_secrets_and_last_workspace() {
        let mut store = DataStore::load().expect("load");
        let old_id = uuid::Uuid::new_v4().to_string().replace('-', "");
        let new_id = "renamed-workspace".to_string();
        store.add(temp_profile(&old_id)).expect("add");
        store
            .set_workspace_secret(&old_id, "bearer_token", "secret-1")
            .expect("set1");
        store
            .set_workspace_secret(&old_id, "actions_api_key", "secret-2")
            .expect("set2");
        let mut settings = store.settings();
        settings.last_workspace_id = old_id.clone();
        store.update_settings(settings).expect("set last workspace");

        let renamed = store
            .rename_workspace_id(&old_id, &new_id)
            .expect("rename");
        assert_eq!(renamed.id, new_id);

        // secrets 已迁移到新 ID，旧 ID 下不再存在
        assert_eq!(
            store
                .get_workspace_secret(&new_id, "bearer_token")
                .expect("get1")
                .as_deref(),
            Some("secret-1")
        );
        assert_eq!(
            store
                .get_workspace_secret(&new_id, "actions_api_key")
                .expect("get2")
                .as_deref(),
            Some("secret-2")
        );
        assert_eq!(
            store
                .get_workspace_secret(&old_id, "bearer_token")
                .expect("get-old")
                .as_deref(),
            None
        );
        // last_workspace_id 同步更新
        assert_eq!(store.settings().last_workspace_id, new_id);

        store.remove(&new_id).expect("cleanup");
    }

    #[test]
    fn rename_workspace_id_is_noop_when_same_id() {
        let mut store = DataStore::load().expect("load");
        let id = uuid::Uuid::new_v4().to_string().replace('-', "");
        store.add(temp_profile(&id)).expect("add");
        store
            .set_workspace_secret(&id, "bearer_token", "secret-x")
            .expect("set");

        let renamed = store.rename_workspace_id(&id, &id).expect("noop");
        assert_eq!(renamed.id, id);
        // secrets 保留在旧 ID 下
        assert_eq!(
            store
                .get_workspace_secret(&id, "bearer_token")
                .expect("get")
                .as_deref(),
            Some("secret-x")
        );

        store.remove(&id).expect("cleanup");
    }

    #[test]
    fn rename_workspace_id_rejects_existing_new_id() {
        let mut store = DataStore::load().expect("load");
        let a_id = uuid::Uuid::new_v4().to_string().replace('-', "");
        let b_id = "occupied-id".to_string();
        store.add(temp_profile(&a_id)).expect("add a");
        store.add(temp_profile(&b_id)).expect("add b");

        let result = store.rename_workspace_id(&a_id, &b_id);
        assert!(result.is_err(), "与已有 ID 冲突应报错");

        store.remove(&a_id).expect("rm a");
        store.remove(&b_id).expect("rm b");
    }
}
