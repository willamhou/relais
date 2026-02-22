use relais_core::vault::Vault;
use tempfile::tempdir;

#[test]
fn vault_stores_and_retrieves_credential() {
    let dir = tempdir().unwrap();
    let vault = Vault::open(dir.path(), "master-password").unwrap();

    vault.store("github", "ghp_abc123token").unwrap();
    let retrieved = vault.retrieve("github").unwrap();
    assert_eq!(retrieved, Some("ghp_abc123token".to_string()));
}

#[test]
fn vault_returns_none_for_missing() {
    let dir = tempdir().unwrap();
    let vault = Vault::open(dir.path(), "master-password").unwrap();

    let retrieved = vault.retrieve("nonexistent").unwrap();
    assert_eq!(retrieved, None);
}

#[test]
fn vault_deletes_credential() {
    let dir = tempdir().unwrap();
    let vault = Vault::open(dir.path(), "master-password").unwrap();

    vault.store("github", "ghp_abc123token").unwrap();
    vault.delete("github").unwrap();
    let retrieved = vault.retrieve("github").unwrap();
    assert_eq!(retrieved, None);
}

#[test]
fn vault_lists_stored_sites() {
    let dir = tempdir().unwrap();
    let vault = Vault::open(dir.path(), "master-password").unwrap();

    vault.store("github", "token1").unwrap();
    vault.store("hackernews", "token2").unwrap();

    let mut sites = vault.list().unwrap();
    sites.sort();
    assert_eq!(sites.len(), 2);
    assert!(sites.contains(&"github".to_string()));
    assert!(sites.contains(&"hackernews".to_string()));
}
