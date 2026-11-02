//! HTTP-level tests for role-based account management.
//!
//! These exercise the real axum router (via `tower::ServiceExt::oneshot`), a real
//! SQLite database and real JWTs, so they cover the middleware + route wiring.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use neuxdot_core::auth::{create_token, hash_password, verify_password};
use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use neuxdot_core::db::schema::ROLE_ROOT;
use neuxdot_server::routes::build_router;
use neuxdot_server::state::AppState;
use serde_json::{json, Value};
use std::sync::Arc;
use tower_service::Service;

const ROOT_PASSWORD: &str = "RootPassw0rd";
const MEMBER_PASSWORD: &str = "MemberPassw0rd";

struct TestApp {
    app: Router,
    db: Arc<Database>,
    root_id: String,
    root_token: String,
    member_id: String,
    member_token: String,
}

async fn setup() -> TestApp {
    let pool = init_db("sqlite::memory:").await.expect("init db");
    let db = Arc::new(Database::new(pool));

    let (root, _derived) = db.get_or_init_root_account().await.expect("root account");
    // Give root a known password so tests don't depend on the derived one.
    let root_hash = hash_password(ROOT_PASSWORD).expect("hash");
    db.update_account_password(&root.id, &root_hash)
        .await
        .expect("set root password");

    let member_hash = hash_password(MEMBER_PASSWORD).expect("hash");
    let member = db
        .create_account("alice", &member_hash)
        .await
        .expect("member account");

    let root_token = create_token(&root.id, &root.username, &root.role).expect("root token");
    let member_token =
        create_token(&member.id, &member.username, &member.role).expect("member token");

    let (event_tx, _rx) = tokio::sync::broadcast::channel(16);
    let app = build_router(AppState {
        db: db.clone(),
        event_tx,
    });

    TestApp {
        app,
        db,
        root_id: root.id,
        root_token,
        member_id: member.id,
        member_token,
    }
}

impl TestApp {
    async fn request(
        &self,
        method: &str,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if !token.is_empty() {
            builder = builder.header("authorization", format!("Bearer {}", token));
        }
        let body = match body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };

        let request = builder.body(body).expect("request");

        let response = self
            .app
            .clone()
            .call(request)
            .await
            .expect("infallible");

        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .expect("body");
        let value: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, value)
    }

    async fn create_member(&self, username: &str) -> String {
        let (status, body) = self
            .request(
                "POST",
                "/api/accounts",
                &self.root_token,
                Some(json!({ "username": username, "password": "SomePassw0rd" })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "create member: {:?}", body);
        body["id"].as_str().expect("id").to_string()
    }
}

#[tokio::test]
async fn member_is_rejected_from_account_management() {
    let t = setup().await;

    let (status, _) = t
        .request("GET", "/api/accounts", &t.member_token, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member must not list accounts");

    let (status, _) = t
        .request(
            "POST",
            "/api/accounts",
            &t.member_token,
            Some(json!({ "username": "mallory", "password": "MalloryPassw0rd" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member must not create accounts");

    let (status, _) = t
        .request(
            "PATCH",
            &format!("/api/accounts/{}", t.root_id),
            &t.member_token,
            Some(json!({ "role": "member" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member must not edit accounts");

    let (status, _) = t
        .request(
            "DELETE",
            &format!("/api/accounts/{}", t.root_id),
            &t.member_token,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member must not delete accounts");

    // A member may change their own password (self-service path stays open).
    let (status, body) = t
        .request(
            "POST",
            &format!("/api/accounts/{}/password", t.member_id),
            &t.member_token,
            Some(json!({
                "old_password": MEMBER_PASSWORD,
                "new_password": "NewMemberPassw0rd",
                "confirm_password": "NewMemberPassw0rd"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "member self password change: {:?}", body);

    // ...but not somebody else's.
    let (status, _) = t
        .request(
            "POST",
            &format!("/api/accounts/{}/password", t.root_id),
            &t.member_token,
            Some(json!({
                "old_password": MEMBER_PASSWORD,
                "new_password": "HijackedPassw0rd",
                "confirm_password": "HijackedPassw0rd"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member must not reset root password");
}

#[tokio::test]
async fn last_root_cannot_be_demoted_or_deleted() {
    let t = setup().await;

    let (status, body) = t
        .request(
            "PATCH",
            &format!("/api/accounts/{}", t.root_id),
            &t.root_token,
            Some(json!({ "role": "member" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "last root demotion: {:?}", body);

    let (status, body) = t
        .request(
            "DELETE",
            &format!("/api/accounts/{}", t.root_id),
            &t.root_token,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "self deletion: {:?}", body);

    // Still exactly one root.
    let root_count = t.db.count_accounts_with_role(ROLE_ROOT).await.expect("count");
    assert_eq!(root_count, 1);

    // A second root may be demoted while the first root remains...
    let second_root_id = t
        .request(
            "POST",
            "/api/accounts",
            &t.root_token,
            Some(json!({
                "username": "bob",
                "password": "SecondRootPassw0rd",
                "role": "root"
            })),
        )
        .await
        .1["id"]
        .as_str()
        .expect("id")
        .to_string();
    let (status, body) = t
        .request(
            "PATCH",
            &format!("/api/accounts/{}", second_root_id),
            &t.root_token,
            Some(json!({ "role": "member" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "demote non-last root: {:?}", body);
    assert_eq!(body["role"], "member");

    // ...and with two roots, another root may be deleted.
    let third_root_id = t
        .request(
            "POST",
            "/api/accounts",
            &t.root_token,
            Some(json!({
                "username": "carol",
                "password": "ThirdRootPassw0rd",
                "role": "root"
            })),
        )
        .await
        .1["id"]
        .as_str()
        .expect("id")
        .to_string();
    let (status, _) = t
        .request(
            "DELETE",
            &format!("/api/accounts/{}", third_root_id),
            &t.root_token,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        t.db.count_accounts_with_role(ROLE_ROOT).await.expect("count"),
        1
    );
}

#[tokio::test]
async fn password_change_validation_and_effects() {
    let t = setup().await;
    let uri = format!("/api/accounts/{}/password", t.member_id);

    // Wrong old password -> 401
    let (status, body) = t
        .request(
            "POST",
            &uri,
            &t.member_token,
            Some(json!({
                "old_password": "TotallyWrong1",
                "new_password": "NewMemberPassw0rd",
                "confirm_password": "NewMemberPassw0rd"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "wrong old password: {:?}", body);

    // Mismatched confirmation -> 400
    let (status, body) = t
        .request(
            "POST",
            &uri,
            &t.member_token,
            Some(json!({
                "old_password": MEMBER_PASSWORD,
                "new_password": "NewMemberPassw0rd",
                "confirm_password": "DifferentPassw0rd"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "mismatch: {:?}", body);

    // Weak password -> 400
    let (status, body) = t
        .request(
            "POST",
            &uri,
            &t.member_token,
            Some(json!({
                "old_password": MEMBER_PASSWORD,
                "new_password": "short",
                "confirm_password": "short"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "weak password: {:?}", body);

    // Successful change updates the stored hash.
    let (status, body) = t
        .request(
            "POST",
            &uri,
            &t.member_token,
            Some(json!({
                "old_password": MEMBER_PASSWORD,
                "new_password": "NewMemberPassw0rd",
                "confirm_password": "NewMemberPassw0rd"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "successful change: {:?}", body);

    let account = t
        .db
        .get_account_by_id(&t.member_id)
        .await
        .expect("lookup")
        .expect("account");
    assert!(verify_password("NewMemberPassw0rd", &account.password_hash));
    assert!(!verify_password(MEMBER_PASSWORD, &account.password_hash));

    // The login endpoint agrees.
    let (status, _) = t
        .request(
            "POST",
            "/api/auth/login",
            "",
            Some(json!({ "username": "alice", "password": MEMBER_PASSWORD })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "old password must stop working");
    let (status, _) = t
        .request(
            "POST",
            "/api/auth/login",
            "",
            Some(json!({ "username": "alice", "password": "NewMemberPassw0rd" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "new password must work");

    // Root may reset a password without the old one.
    let (status, body) = t
        .request(
            "POST",
            &uri,
            &t.root_token,
            Some(json!({
                "new_password": "RootResetPassw0rd",
                "confirm_password": "RootResetPassw0rd"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "root admin reset: {:?}", body);
    let account = t
        .db
        .get_account_by_id(&t.member_id)
        .await
        .expect("lookup")
        .expect("account");
    assert!(verify_password("RootResetPassw0rd", &account.password_hash));
}

#[tokio::test]
async fn root_can_create_edit_and_delete_member() {
    let t = setup().await;

    // Create
    let (status, created) = t
        .request(
            "POST",
            "/api/accounts",
            &t.root_token,
            Some(json!({
                "username": "dave",
                "password": "DavePassw0rd",
                "display_name": "Dave the Bot"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create: {:?}", created);
    let dave_id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["username"], "dave");
    assert_eq!(created["role"], "member");
    assert_eq!(created["display_name"], "Dave the Bot");

    // Duplicate username -> 409
    let (status, _) = t
        .request(
            "POST",
            "/api/accounts",
            &t.root_token,
            Some(json!({ "username": "dave", "password": "DavePassw0rd" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Invalid role -> 400
    let (status, _) = t
        .request(
            "POST",
            "/api/accounts",
            &t.root_token,
            Some(json!({ "username": "eve", "password": "EvePassw0rd", "role": "admin" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // List includes root and member with expected JSON shape.
    let (status, list) = t.request("GET", "/api/accounts", &t.root_token, None).await;
    assert_eq!(status, StatusCode::OK);
    let list = list.as_array().expect("array");
    assert_eq!(list.len(), 3, "root + alice + dave: {:?}", list);
    assert_eq!(list[0]["username"], "root", "root listed first");
    let entry = list
        .iter()
        .find(|a| a["id"] == json!(dave_id))
        .expect("dave listed");
    for key in [
        "id",
        "username",
        "role",
        "display_name",
        "avatar_url",
        "created_at",
    ] {
        assert!(entry.get(key).is_some(), "missing key {} in {:?}", key, entry);
    }

    // Edit username / display name / avatar
    let (status, updated) = t
        .request(
            "PATCH",
            &format!("/api/accounts/{}", dave_id),
            &t.root_token,
            Some(json!({
                "username": "dave2",
                "display_name": "Dave Two",
                "avatar_url": "data:image/png;base64,AAAA"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "patch: {:?}", updated);
    assert_eq!(updated["username"], "dave2");
    assert_eq!(updated["display_name"], "Dave Two");
    assert_eq!(updated["avatar_url"], "data:image/png;base64,AAAA");

    // Promote to root, then delete (no longer the last root).
    let (status, promoted) = t
        .request(
            "PATCH",
            &format!("/api/accounts/{}", dave_id),
            &t.root_token,
            Some(json!({ "role": "root" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(promoted["role"], "root");

    let (status, _) = t
        .request(
            "DELETE",
            &format!("/api/accounts/{}", dave_id),
            &t.root_token,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(t
        .db
        .get_account_by_id(&dave_id)
        .await
        .expect("lookup")
        .is_none());

    // Delete of a plain member works and is idempotent-safe (404 afterwards).
    let member_id = t.create_member("frank").await;
    let (status, _) = t
        .request(
            "DELETE",
            &format!("/api/accounts/{}", member_id),
            &t.root_token,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = t
        .request(
            "DELETE",
            &format!("/api/accounts/{}", member_id),
            &t.root_token,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn profile_update_accepts_avatar_url() {
    let t = setup().await;

    let (status, body) = t
        .request(
            "PUT",
            "/api/profile",
            &t.member_token,
            Some(json!({
                "display_name": "Alice A.",
                "avatar_url": "https://example.com/alice.png"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "profile update: {:?}", body);
    assert_eq!(body["display_name"], "Alice A.");
    assert_eq!(body["avatar_url"], "https://example.com/alice.png");

    let (status, me) = t.request("GET", "/api/auth/me", &t.member_token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["role"], "member");
    assert_eq!(me["profile"]["avatar_url"], "https://example.com/alice.png");

    // Empty string clears the custom picture.
    let (status, cleared) = t
        .request(
            "PUT",
            "/api/profile",
            &t.member_token,
            Some(json!({ "avatar_url": "" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(cleared["avatar_url"].is_null(), "cleared: {:?}", cleared);
}

#[tokio::test]
async fn jwt_role_claim_is_enforced() {
    let t = setup().await;

    // Authority is resolved from the database, not the token claim.
    // A token that *claims* membership for an account that really is root is
    // still allowed: the claim cannot be used to strip or grant privileges.
    // (The complementary case — a token claiming root for a demoted account —
    // is covered by `demoted_root_loses_admin_rights_immediately`.)
    let downgraded = create_token(&t.root_id, "root", "member").expect("token");
    let (status, _) = t.request("GET", "/api/accounts", &downgraded, None).await;
    assert_eq!(status, StatusCode::OK);

    // A member account cannot administer even if its token claims root.
    let forged = create_token(&t.member_id, "alice", ROLE_ROOT).expect("token");
    let (status, _) = t.request("GET", "/api/accounts", &forged, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Login returns the role so clients can render admin UI.
    let (status, body) = t
        .request(
            "POST",
            "/api/auth/login",
            "",
            Some(json!({ "username": "root", "password": ROOT_PASSWORD })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "login: {:?}", body);
    assert_eq!(body["role"], "root");
}

/// SECURITY REGRESSION: authority must come from the database, not the JWT.
///
/// Tokens live for 30 days. If `require_root` trusted the `role` claim, a root
/// demoted to member (or a deleted account) would keep full admin rights until
/// the token expired. The role must be re-resolved on every request.
#[tokio::test]
async fn demoted_root_loses_admin_rights_immediately() {
    let app = setup().await;

    // Sanity: the root token works while the account is root.
    let (status, _) = app.request("GET", "/api/accounts", &app.root_token, None).await;
    assert_eq!(status, StatusCode::OK);

    // Create a second root so the first one may legally be demoted.
    let other_hash = hash_password("OtherRootPass1").expect("hash");
    let other = app
        .db
        .create_account("root2", &other_hash)
        .await
        .expect("second root");
    app.db
        .update_account(&other.id, None, Some(ROLE_ROOT))
        .await
        .expect("promote second root");

    // Demote the original root directly in the database.
    app.db
        .update_account(&app.root_id, None, Some("member"))
        .await
        .expect("demote root");

    // The OLD token still carries role="root" in its claims, but the account is
    // now a member: admin access must be refused.
    let (status, body) = app.request("GET", "/api/accounts", &app.root_token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "stale root claim must not grant admin access, got body: {}",
        body
    );
}

/// A deleted account's token must stop working entirely.
#[tokio::test]
async fn deleted_account_token_is_rejected() {
    let app = setup().await;

    // Member can reach their own profile while they exist.
    let (status, _) = app.request("GET", "/api/profile", &app.member_token, None).await;
    assert_eq!(status, StatusCode::OK);

    app.db
        .delete_account(&app.member_id)
        .await
        .expect("delete member");

    let (status, _) = app.request("GET", "/api/profile", &app.member_token, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "deleted account token must be rejected");
}
