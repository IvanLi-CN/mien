//! 代理配置存储：加密往返、AAD 绑定、CRUD、分页与落盘明文检查。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mien_core::ErrorCode;
use mien_core::ports::proxy::{
    ProxyId, ProxyInput, ProxyKind, ProxyPatch, ProxyRepository, ProxyView,
};
use mien_core::secrets::{FieldAad, MasterKey, decrypt_field, encrypt_field};
use mien_core::storage::open_and_migrate;
use mien_core::storage::proxy::SqliteProxyRepository;
use serde_json::json;

const PASSWORD: &str = "hunter2-not-in-db";
const UUID_VALUE: &str = "9d3a1d0e-0000-4000-8000-00000000beef";

fn key() -> MasterKey {
    MasterKey::from_bytes([7u8; 32])
}

fn other_key() -> MasterKey {
    MasterKey::from_bytes([9u8; 32])
}

fn aad<'a>(proxy_id: &'a str, field: &'a str) -> FieldAad<'a> {
    FieldAad {
        proxy_id,
        field_name: field,
    }
}

fn workspace() -> PathBuf {
    std::env::temp_dir().join(format!("mien-proxy-store-{}", ProxyId::new().0))
}

fn setup() -> (PathBuf, SqliteProxyRepository) {
    let root = workspace();
    std::fs::create_dir_all(&root).expect("temp dir");
    let pool = open_and_migrate(&root.join("db.sqlite")).expect("migrate");
    let repository = SqliteProxyRepository::new(pool, key());
    (root, repository)
}

fn input(name: &str) -> ProxyInput {
    let mut options = BTreeMap::new();
    options.insert("transport".to_owned(), json!("ws"));
    options.insert("tls".to_owned(), json!(true));
    let mut credentials = BTreeMap::new();
    credentials.insert("password".to_owned(), PASSWORD.to_owned());
    credentials.insert("uuid".to_owned(), UUID_VALUE.to_owned());
    ProxyInput {
        name: name.to_owned(),
        kind: ProxyKind::Vless,
        server: "edge.example.test".to_owned(),
        port: 443,
        options,
        credentials,
    }
}

#[test]
fn encrypt_decrypt_round_trip_binds_proxy_and_field() {
    let plaintext = b"super-secret-token";
    let sealed = encrypt_field(&key(), aad("01PROXY", "password"), plaintext).expect("encrypt");
    assert_eq!(sealed.len(), 12 + plaintext.len() + 16, "nonce||ct||tag");
    assert_eq!(
        decrypt_field(&key(), aad("01PROXY", "password"), &sealed).expect("decrypt"),
        plaintext
    );

    // 搬到另一记录、或另一字段，都必须认证失败。
    assert_eq!(
        decrypt_field(&key(), aad("02OTHER", "password"), &sealed)
            .expect_err("moved record")
            .code(),
        ErrorCode::StorageIntegrityFailed
    );
    assert_eq!(
        decrypt_field(&key(), aad("01PROXY", "uuid"), &sealed)
            .expect_err("moved field")
            .code(),
        ErrorCode::StorageIntegrityFailed
    );
    assert_eq!(
        decrypt_field(&other_key(), aad("01PROXY", "password"), &sealed)
            .expect_err("wrong key")
            .code(),
        ErrorCode::StorageIntegrityFailed
    );

    // 同一输入两次加密的 nonce 不同，密文也不同。
    let again = encrypt_field(&key(), aad("01PROXY", "password"), plaintext).expect("encrypt");
    assert_ne!(sealed, again);
    assert_ne!(&sealed[..12], &again[..12]);
}

#[test]
fn master_key_parses_hex_and_hides_bytes() {
    let hex = "11".repeat(32);
    let parsed = MasterKey::parse_hex(&hex).expect("parse");
    assert_eq!(format!("{parsed:?}"), "MasterKey { bytes: \"<redacted>\" }");
    assert!(MasterKey::parse_hex(&format!("0x{}", hex.to_uppercase())).is_ok());
    assert_eq!(
        MasterKey::parse_hex("zz").expect_err("bad key").code(),
        ErrorCode::InternalUnexpected
    );
    assert_eq!(
        MasterKey::parse_hex(&hex[..62])
            .expect_err("short key")
            .code(),
        ErrorCode::InternalUnexpected
    );
    // 相同密钥材料产生可互解的密文。
    let sealed = encrypt_field(&parsed, aad("01PROXY", "password"), b"x").expect("encrypt");
    let reparsed = MasterKey::parse_hex(&hex).expect("parse again");
    assert_eq!(
        decrypt_field(&reparsed, aad("01PROXY", "password"), &sealed).expect("decrypt"),
        b"x"
    );
}

#[cfg(unix)]
#[test]
fn master_key_file_requires_0600_and_reads_hex() {
    use std::os::unix::fs::PermissionsExt;

    let root = workspace();
    let secrets_dir = root.join("secrets");
    std::fs::create_dir_all(&secrets_dir).expect("secrets dir");
    let path = secrets_dir.join("master.key");
    std::fs::write(&path, format!("{}\n", "ab".repeat(32))).expect("write key");

    // 默认 umask 下的 0644 必须被拒绝。
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod 644");
    assert_eq!(
        MasterKey::from_file(&root)
            .expect_err("world readable")
            .code(),
        ErrorCode::InternalUnexpected
    );

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod 600");
    let loaded = MasterKey::from_file(&root).expect("0600 file");
    let expected = MasterKey::parse_hex(&"ab".repeat(32)).expect("parse");
    let sealed = encrypt_field(&loaded, aad("01PROXY", "password"), b"x").expect("encrypt");
    assert_eq!(
        decrypt_field(&expected, aad("01PROXY", "password"), &sealed).expect("decrypt"),
        b"x"
    );

    // 指向同一密钥的符号链接也必须拒绝，避免主密钥被重定向到数据目录之外。
    let real = root.join("real.key");
    std::fs::rename(&path, &real).expect("move key");
    std::os::unix::fs::symlink(&real, &path).expect("symlink");
    assert_eq!(
        MasterKey::from_file(&root)
            .expect_err("symlinked key")
            .code(),
        ErrorCode::InternalUnexpected
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn create_get_and_roundtrip_through_storage() {
    let (root, repository) = setup();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let view = repository.create(input("primary")).await.expect("create");
        assert_eq!(view.name, "primary");
        assert_eq!(view.kind, ProxyKind::Vless);
        assert_eq!(view.server, "edge.example.test");
        assert_eq!(view.port, 443);
        assert_eq!(view.options.get("transport"), Some(&json!("ws")));
        assert!(view.credential_set);
        assert_eq!(view.credential_fields, ["password", "uuid"]);

        let fetched = repository.get(&view.id).await.expect("get");
        assert_eq!(fetched.id, view.id);
        assert_eq!(fetched.credential_fields, view.credential_fields);

        let secret = repository.load_secret(&view.id).await.expect("secret");
        assert_eq!(
            secret.credentials.get("password").map(String::as_str),
            Some(PASSWORD)
        );
        assert_eq!(
            secret.credentials.get("uuid").map(String::as_str),
            Some(UUID_VALUE)
        );
        assert_eq!(secret.options.get("transport"), Some(&json!("ws")));
        // 明文不出现在 Debug。
        let debug = format!("{secret:?}");
        assert!(!debug.contains(PASSWORD), "Debug leaked password: {debug}");
        assert!(!debug.contains(UUID_VALUE), "Debug leaked uuid: {debug}");
        assert!(debug.contains("credential_fields"));
    });
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn missing_proxy_reports_proxy_not_found() {
    let (root, repository) = setup();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let missing = ProxyId("01MISSINGPROXY0000000000000".to_owned());
        assert_eq!(
            repository.get(&missing).await.expect_err("get").code(),
            ErrorCode::ProxyNotFound
        );
        assert_eq!(
            repository
                .update(&missing, ProxyPatch::default())
                .await
                .expect_err("update")
                .code(),
            ErrorCode::ProxyNotFound
        );
        assert_eq!(
            repository
                .delete(&missing)
                .await
                .expect_err("delete")
                .code(),
            ErrorCode::ProxyNotFound
        );
    });
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn update_merges_fields_and_replaces_credentials() {
    let (root, repository) = setup();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let created = repository.create(input("primary")).await.expect("create");
        let mut credentials = BTreeMap::new();
        credentials.insert("password".to_owned(), "rotated-secret".to_owned());
        let updated = repository
            .update(
                &created.id,
                ProxyPatch {
                    name: Some("renamed".to_owned()),
                    credentials: Some(credentials),
                    ..ProxyPatch::default()
                },
            )
            .await
            .expect("update");
        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.server, created.server);
        assert_eq!(updated.options, created.options);
        assert_eq!(updated.credential_fields, ["password"]);

        let secret = repository.load_secret(&created.id).await.expect("secret");
        assert_eq!(secret.credentials.len(), 1);
        assert_eq!(
            secret.credentials.get("password").map(String::as_str),
            Some("rotated-secret")
        );

        // 只改端口时凭据保持不变。
        let port_only = repository
            .update(
                &created.id,
                ProxyPatch {
                    port: Some(8443),
                    ..ProxyPatch::default()
                },
            )
            .await
            .expect("port update");
        assert_eq!(port_only.port, 8443);
        assert_eq!(port_only.credential_fields, ["password"]);
    });
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn list_paginates_by_id_cursor() {
    let (root, repository) = setup();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let mut created = Vec::new();
        for index in 0..5 {
            created.push(
                repository
                    .create(input(&format!("proxy-{index}")))
                    .await
                    .expect("create"),
            );
        }
        created.sort_by(|a, b| a.id.cmp(&b.id));
        let expected: Vec<ProxyId> = created.iter().map(|view| view.id.clone()).collect();

        let first = repository.list(Some(2), None).await.expect("first page");
        assert_eq!(ids(&first.items), expected[..2].to_vec());
        let cursor = first.next_cursor.clone().expect("next cursor");
        assert_eq!(cursor, expected[1].0);

        let second = repository
            .list(Some(2), Some(cursor))
            .await
            .expect("second page");
        assert_eq!(ids(&second.items), expected[2..4].to_vec());

        let third = repository
            .list(Some(2), second.next_cursor.clone())
            .await
            .expect("third page");
        assert_eq!(ids(&third.items), expected[4..].to_vec());
        assert!(third.next_cursor.is_none());
    });
    let _ = std::fs::remove_dir_all(root);
}

fn ids(items: &[ProxyView]) -> Vec<ProxyId> {
    items.iter().map(|view| view.id.clone()).collect()
}

#[test]
fn delete_removes_row_and_reports_missing_afterwards() {
    let (root, repository) = setup();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let created = repository.create(input("doomed")).await.expect("create");
        repository.delete(&created.id).await.expect("delete");
        assert_eq!(
            repository.get(&created.id).await.expect_err("gone").code(),
            ErrorCode::ProxyNotFound
        );
    });
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn invalid_kind_and_secret_options_are_rejected() {
    let (root, repository) = setup();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        // DDL 枚举外的 kind 无法用类型构造，直接验证类型解析入口。
        assert_eq!(
            ProxyKind::parse("tuic").expect_err("tuic").code(),
            ErrorCode::ProxyConfigInvalid
        );
        assert!(ProxyKind::ALL.iter().all(|kind| kind.as_str() != "tuic"));

        let mut leaked = input("leaky");
        leaked.options.insert("apiToken".to_owned(), json!("t"));
        assert_eq!(
            repository
                .create(leaked)
                .await
                .expect_err("secret option key")
                .code(),
            ErrorCode::ProxyConfigInvalid
        );

        let mut nested = input("nested");
        nested.options.insert("extra".to_owned(), json!({"a": 1}));
        assert_eq!(
            repository
                .create(nested)
                .await
                .expect_err("nested option")
                .code(),
            ErrorCode::ProxyConfigInvalid
        );

        let mut blank = input("blank");
        blank.name = "  ".to_owned();
        assert_eq!(
            repository
                .create(blank)
                .await
                .expect_err("blank name")
                .code(),
            ErrorCode::ProxyConfigInvalid
        );
    });
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn credentials_never_appear_in_database_bytes() {
    let (root, repository) = setup();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let (view_debug, view_json) = runtime.block_on(async {
        let view = repository
            .create(input("plaintext-check"))
            .await
            .expect("create");
        let view_json = serde_json::to_string(&view).expect("serialize view");
        (format!("{view:?}"), view_json)
    });
    drop(repository);
    for text in [view_debug, view_json] {
        assert!(!text.contains(PASSWORD), "view leaked password: {text}");
        assert!(!text.contains(UUID_VALUE), "view leaked uuid: {text}");
    }

    // WAL 也可能是唯一载体，因此一并检查三个文件。
    for suffix in ["", "-wal", "-shm"] {
        let path = Path::new(&root).join(format!("db.sqlite{suffix}"));
        if !path.exists() {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read db bytes");
        for needle in [PASSWORD, UUID_VALUE] {
            assert!(
                !contains(&bytes, needle.as_bytes()),
                "{} contains plaintext {needle}",
                path.display()
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn moved_ciphertext_between_rows_fails_to_load() {
    let (root, repository) = setup();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let first = repository.create(input("first")).await.expect("create");
        let second = repository.create(input("second")).await.expect("create");
        let ciphertext: Vec<u8> =
            sqlx::query_scalar("SELECT config_ciphertext FROM proxies WHERE id=?1")
                .bind(&second.id.0)
                .fetch_one(repository.pool())
                .await
                .expect("select ciphertext");
        // 把 second 的密文搬到 first 行：信封内每个字段的 AAD 都绑定 second 的 id。
        sqlx::query("UPDATE proxies SET config_ciphertext=?1 WHERE id=?2")
            .bind(&ciphertext)
            .bind(&first.id.0)
            .execute(repository.pool())
            .await
            .expect("swap ciphertext");
        assert_eq!(
            repository
                .get(&first.id)
                .await
                .expect_err("moved ciphertext")
                .code(),
            ErrorCode::StorageIntegrityFailed
        );
    });
    let _ = std::fs::remove_dir_all(root);
}
