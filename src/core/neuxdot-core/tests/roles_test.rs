//! Role bootstrap + migration behaviour for accounts/profiles.

use neuxdot_core::auth::{hash_password, verify_password};
use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::{is_valid_role, ROLE_MEMBER, ROLE_ROOT};

#[tokio::test]
async fn root_bootstrap_creates_root_role() -> Result<(), Box<dyn std::error::Error>> {
    let pool = init_db("sqlite::memory:").await?;
    let db = Database::new(pool);

    let (root, password) = db.get_or_init_root_account().await?;
    assert_eq!(root.username, "root");
    assert_eq!(root.role, ROLE_ROOT);
    assert!(root.is_root());
    assert!(verify_password(&password, &root.password_hash));

    // Idempotent: a second call returns the same account and promotes it if needed.
    let (again, same_password) = db.get_or_init_root_account().await?;
    assert_eq!(again.id, root.id);
    assert_eq!(same_password, password);
    assert_eq!(db.count_accounts_with_role(ROLE_ROOT).await?, 1);

    Ok(())
}

#[tokio::test]
async fn bootstrap_repairs_root_demoted_to_member() -> Result<(), Box<dyn std::error::Error>> {
    let pool = init_db("sqlite::memory:").await?;
    let db = Database::new(pool);

    let (root, _) = db.get_or_init_root_account().await?;
    db.update_account(&root.id, None, Some(ROLE_MEMBER)).await?;
    assert_eq!(db.count_accounts_with_role(ROLE_ROOT).await?, 0);
    assert!(db.is_last_root(&root.id).await? == false);

    let (repaired, _) = db.get_or_init_root_account().await?;
    assert_eq!(repaired.role, ROLE_ROOT);
    assert!(db.is_last_root(&repaired.id).await?);

    Ok(())
}

#[tokio::test]
async fn new_accounts_default_to_member() -> Result<(), Box<dyn std::error::Error>> {
    let pool = init_db("sqlite::memory:").await?;
    let db = Database::new(pool);

    let hash = hash_password("MemberPassw0rd").expect("hash password");
    let account = db.create_account("member1", &hash).await?;
    assert_eq!(account.role, ROLE_MEMBER);
    assert!(!account.is_root());

    let root = db
        .create_account_with_role("root2", &hash, ROLE_ROOT)
        .await?;
    assert_eq!(root.role, ROLE_ROOT);
    assert_eq!(db.count_accounts_with_role(ROLE_ROOT).await?, 1);

    // Role validation only accepts the two known roles.
    assert!(is_valid_role("root"));
    assert!(is_valid_role("member"));
    assert!(!is_valid_role("admin"));

    // Listing puts roots first.
    let listed = db.list_accounts().await?;
    assert_eq!(listed[0].role, ROLE_ROOT);

    Ok(())
}

#[tokio::test]
async fn profile_avatar_url_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let pool = init_db("sqlite::memory:").await?;
    let db = Database::new(pool);

    let hash = hash_password("MemberPassw0rd").expect("hash password");
    let account = db.create_account("avatar-user", &hash).await?;
    let profile = db.get_profile(&account.id).await?.expect("profile");
    assert!(profile.avatar_url.is_none());

    let updated = db
        .update_profile(
            &account.id,
            None,
            None,
            None,
            Some("data:image/png;base64,ZZZZ"),
        )
        .await?;
    assert_eq!(
        updated.avatar_url.as_deref(),
        Some("data:image/png;base64,ZZZZ")
    );
    // Untouched fields survive.
    assert_eq!(updated.display_name.as_deref(), Some("avatar-user"));

    let cleared = db
        .update_profile(&account.id, None, None, None, Some(""))
        .await?;
    assert!(cleared.avatar_url.is_none());

    Ok(())
}

#[tokio::test]
async fn agent_avatar_url_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let pool = init_db("sqlite::memory:").await?;
    let db = Database::new(pool);

    let hash = hash_password("MemberPassw0rd").expect("hash password");
    let account = db.create_account("agent-avatar-user", &hash).await?;

    // create_agent persists an avatar_url when provided...
    let agent = db
        .create_agent(
            &account.id,
            "Painter",
            None,
            None,
            Some("data:image/png;base64,AAAA"),
            "fallback",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    assert_eq!(
        agent.avatar_url.as_deref(),
        Some("data:image/png;base64,AAAA")
    );

    // ...and leaves it NULL when omitted.
    let plain = db
        .create_agent(
            &account.id,
            "Plain",
            None,
            None,
            None,
            "fallback",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    assert!(plain.avatar_url.is_none());

    Ok(())
}

#[tokio::test]
async fn patch_agent_avatar_url_sets_preserves_and_clears() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = init_db("sqlite::memory:").await?;
    let db = Database::new(pool);

    let hash = hash_password("MemberPassw0rd").expect("hash password");
    let account = db.create_account("agent-avatar-patch", &hash).await?;

    let agent = db
        .create_agent(
            &account.id,
            "AvatarBot",
            None,
            None,
            None,
            "fallback",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    assert!(agent.avatar_url.is_none());

    // Setting the picture persists it (other fields untouched).
    let set = db
        .patch_agent(
            &agent.id,
            None,
            None,
            None,
            Some("https://example.com/agent.png"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    assert_eq!(
        set.avatar_url.as_deref(),
        Some("https://example.com/agent.png")
    );
    assert_eq!(set.name, "AvatarBot");

    // Omitting avatar_url preserves the existing value even when other fields change.
    let preserved = db
        .patch_agent(
            &agent.id,
            Some("AvatarBot v2"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    assert_eq!(
        preserved.avatar_url.as_deref(),
        Some("https://example.com/agent.png")
    );
    assert_eq!(preserved.name, "AvatarBot v2");

    // An explicit empty string clears the picture to NULL (not an empty string).
    let cleared = db
        .patch_agent(
            &agent.id,
            None,
            None,
            None,
            Some(""),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    assert!(
        cleared.avatar_url.is_none(),
        "expected NULL, got {:?}",
        cleared.avatar_url
    );

    // And it stays cleared on a subsequent patch that omits the field.
    let still_cleared = db
        .patch_agent(
            &agent.id, None, None, None, None, None, None, None, None, None, None, None, None,
        )
        .await?;
    assert!(still_cleared.avatar_url.is_none());

    Ok(())
}
