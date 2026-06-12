//! Shared test suite executed against both SQLite and PostgreSQL backends.

use flaps_domain::SdkKeyKind;
use flaps_domain::{
    Environment, EnvironmentKey, ExternalRef, Flag, FlagEnvConfig, FlagKey, FlagType, ManagedBy,
    MatchOperator, Predicate, Project, ProjectKey, Segment, SegmentKey, SegmentMatch, ServeTarget,
    TargetingRule, ValueType, VariantKey, VariantValue, Variants, WeightedVariant,
};
use flaps_store::{
    KeyHasher, NewSdkKey, SdkKeyScope,
    repository::{
        EnvironmentRepository, FlagEnvConfigRepository, FlagRepository, ProjectRepository,
        SdkKeyRepository, SegmentRepository, TransactionalStore, WriteSession,
    },
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

pub(crate) fn make_project(key: &str) -> Project {
    Project {
        key: ProjectKey::new(key).unwrap(),
        name: format!("Project {key}"),
        description: Some("test project".into()),
        external_ref: None,
        managed_by: ManagedBy::Local,
    }
}

pub(crate) fn make_project_with_ref(key: &str, ext_ref: &str) -> Project {
    Project {
        key: ProjectKey::new(key).unwrap(),
        name: format!("Project {key}"),
        description: None,
        external_ref: Some(ExternalRef::new(ext_ref)),
        managed_by: ManagedBy::Federated,
    }
}

pub(crate) fn make_env(key: &str) -> Environment {
    Environment {
        key: EnvironmentKey::new(key).unwrap(),
        name: format!("Env {key}"),
        external_ref: None,
        managed_by: ManagedBy::Local,
    }
}

// make_env_with_ref is kept for potential future test cases (external_ref on environments).
#[allow(dead_code)]
pub(crate) fn make_env_with_ref(key: &str, ext_ref: &str) -> Environment {
    Environment {
        key: EnvironmentKey::new(key).unwrap(),
        name: format!("Env {key}"),
        external_ref: Some(ExternalRef::new(ext_ref)),
        managed_by: ManagedBy::Federated,
    }
}

pub(crate) fn make_flag(key: &str) -> Flag {
    let variants = Variants::new(
        ValueType::Boolean,
        [
            (VariantKey::new("on").unwrap(), VariantValue::Bool(true)),
            (VariantKey::new("off").unwrap(), VariantValue::Bool(false)),
        ],
    )
    .unwrap();
    Flag {
        key: FlagKey::new(key).unwrap(),
        name: format!("Flag {key}"),
        description: Some("test flag".into()),
        flag_type: FlagType::Release,
        value_type: ValueType::Boolean,
        variants,
    }
}

pub(crate) fn make_segment(key: &str) -> Segment {
    let match_expr = SegmentMatch::And(vec![
        SegmentMatch::Or(vec![
            SegmentMatch::Predicate(Predicate {
                attribute: "tier".into(),
                operator: MatchOperator::Equals,
                values: vec![serde_json::json!("beta")],
            }),
            SegmentMatch::Predicate(Predicate {
                attribute: "plan".into(),
                operator: MatchOperator::In,
                values: vec![serde_json::json!("pro"), serde_json::json!("enterprise")],
            }),
        ]),
        SegmentMatch::Not(Box::new(SegmentMatch::Predicate(Predicate {
            attribute: "blocked".into(),
            operator: MatchOperator::Equals,
            values: vec![serde_json::json!(true)],
        }))),
    ]);
    Segment {
        key: SegmentKey::new(key).unwrap(),
        name: format!("Segment {key}"),
        match_expr,
    }
}

pub(crate) fn make_flag_env_config() -> FlagEnvConfig {
    FlagEnvConfig {
        enabled: true,
        rules: vec![TargetingRule {
            segments: vec![SegmentKey::new("beta-users").unwrap()],
            serve: ServeTarget::Fixed(VariantKey::new("on").unwrap()),
        }],
        default_rule: ServeTarget::rollout(vec![
            WeightedVariant {
                variant: VariantKey::new("on").unwrap(),
                weight: 10,
            },
            WeightedVariant {
                variant: VariantKey::new("off").unwrap(),
                weight: 90,
            },
        ])
        .unwrap(),
    }
}

// ---------------------------------------------------------------------------
// Shared suite
// ---------------------------------------------------------------------------

/// Runs all 14 acceptance test cases against a store that implements all traits.
pub(crate) async fn run_all<S>(store: S)
where
    S: ProjectRepository
        + EnvironmentRepository
        + FlagRepository
        + SegmentRepository
        + FlagEnvConfigRepository
        + SdkKeyRepository
        + TransactionalStore
        + Clone
        + 'static,
    for<'a> <S as TransactionalStore>::Session<'a>: WriteSession,
{
    test_project_round_trip(&store).await;
    test_project_upsert_is_idempotent(&store).await;
    test_external_ref_unique(&store).await;
    test_external_ref_null_allowed(&store).await;
    test_environment_round_trip(&store).await;
    test_flag_round_trip(&store).await;
    test_segment_round_trip(&store).await;
    test_flag_env_config_round_trip(&store).await;
    test_cascade_delete(&store).await;
    test_sdk_key_lookup_by_raw(&store).await;
    test_sdk_key_pepper_changes_hash();
    test_transaction_commit_persists(&store).await;
    test_transaction_drop_rolls_back(&store).await;
    // sdk_key_is_hashed_at_rest is backend-specific (requires direct DB access);
    // it is tested inline in the sqlite test file via a raw query.
}

// ---------------------------------------------------------------------------
// Test 1: project_round_trip
// ---------------------------------------------------------------------------

async fn test_project_round_trip<S: ProjectRepository>(store: &S) {
    let proj = make_project("round-trip");
    store.upsert_project(&proj).await.unwrap();

    let fetched = store.get_project(&proj.key).await.unwrap().unwrap();
    assert_eq!(fetched.name, proj.name);
    assert_eq!(fetched.managed_by, proj.managed_by);

    let list = store.list_projects().await.unwrap();
    assert!(list.iter().any(|p| p.key == proj.key));

    store.delete_project(&proj.key).await.unwrap();
    let after = store.get_project(&proj.key).await.unwrap();
    assert!(after.is_none(), "project should be gone after delete");
}

// ---------------------------------------------------------------------------
// Test 2: project_upsert_is_idempotent
// ---------------------------------------------------------------------------

async fn test_project_upsert_is_idempotent<S: ProjectRepository>(store: &S) {
    let mut proj = make_project("idempotent");
    store.upsert_project(&proj).await.unwrap();

    proj.name = "Updated Name".into();
    store.upsert_project(&proj).await.unwrap();

    let list = store.list_projects().await.unwrap();
    let matching: Vec<_> = list.iter().filter(|p| p.key == proj.key).collect();
    assert_eq!(matching.len(), 1, "should have exactly one project");
    assert_eq!(matching[0].name, "Updated Name");

    store.delete_project(&proj.key).await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 3: external_ref_unique
// ---------------------------------------------------------------------------

async fn test_external_ref_unique<S: ProjectRepository>(store: &S) {
    let proj_a = make_project_with_ref("ext-a", "urn:shared:ref");
    let proj_b = make_project_with_ref("ext-b", "urn:shared:ref");

    store.upsert_project(&proj_a).await.unwrap();
    let result = store.upsert_project(&proj_b).await;

    assert!(
        result.is_err(),
        "second project with same external_ref must fail"
    );
    if let Err(flaps_store::StoreError::Conflict(_)) = result {
        // expected
    } else {
        panic!("expected Conflict error, got: {result:?}");
    }

    store.delete_project(&proj_a.key).await.unwrap();
    let _ = store.delete_project(&proj_b.key).await;
}

// ---------------------------------------------------------------------------
// Test 4: external_ref_null_allowed
// ---------------------------------------------------------------------------

async fn test_external_ref_null_allowed<S: ProjectRepository>(store: &S) {
    let proj_c = make_project("null-ref-c");
    let proj_d = make_project("null-ref-d");

    store.upsert_project(&proj_c).await.unwrap();
    store.upsert_project(&proj_d).await.unwrap();

    let list = store.list_projects().await.unwrap();
    assert!(list.iter().any(|p| p.key == proj_c.key));
    assert!(list.iter().any(|p| p.key == proj_d.key));

    store.delete_project(&proj_c.key).await.unwrap();
    store.delete_project(&proj_d.key).await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 5: environment_round_trip (scoping by project)
// ---------------------------------------------------------------------------

async fn test_environment_round_trip<S: ProjectRepository + EnvironmentRepository>(store: &S) {
    let proj1 = make_project("env-proj1");
    let proj2 = make_project("env-proj2");
    store.upsert_project(&proj1).await.unwrap();
    store.upsert_project(&proj2).await.unwrap();

    let env = make_env("production");
    store.upsert_environment(&proj1.key, &env).await.unwrap();
    store.upsert_environment(&proj2.key, &env).await.unwrap();

    let e1 = store
        .get_environment(&proj1.key, &env.key)
        .await
        .unwrap()
        .unwrap();
    let e2 = store
        .get_environment(&proj2.key, &env.key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e1.key, env.key);
    assert_eq!(e2.key, env.key);

    let list1 = store.list_environments(&proj1.key).await.unwrap();
    let list2 = store.list_environments(&proj2.key).await.unwrap();
    assert_eq!(list1.len(), 1);
    assert_eq!(list2.len(), 1);

    store
        .delete_environment(&proj1.key, &env.key)
        .await
        .unwrap();
    let after = store.get_environment(&proj1.key, &env.key).await.unwrap();
    assert!(after.is_none());

    store.delete_project(&proj1.key).await.unwrap();
    store.delete_project(&proj2.key).await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 6: flag_round_trip
// ---------------------------------------------------------------------------

async fn test_flag_round_trip<S: ProjectRepository + FlagRepository>(store: &S) {
    let proj = make_project("flag-proj");
    store.upsert_project(&proj).await.unwrap();

    let flag = make_flag("my-flag");
    store.upsert_flag(&proj.key, &flag).await.unwrap();

    let fetched = store.get_flag(&proj.key, &flag.key).await.unwrap().unwrap();
    assert_eq!(fetched.name, flag.name);
    assert_eq!(fetched.flag_type, flag.flag_type);
    assert_eq!(fetched.value_type, flag.value_type);
    assert_eq!(
        fetched.variants, flag.variants,
        "variants must round-trip faithfully"
    );

    let list = store.list_flags(&proj.key).await.unwrap();
    assert_eq!(list.len(), 1);

    store.delete_flag(&proj.key, &flag.key).await.unwrap();
    assert!(
        store
            .get_flag(&proj.key, &flag.key)
            .await
            .unwrap()
            .is_none()
    );

    store.delete_project(&proj.key).await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 7: segment_round_trip
// ---------------------------------------------------------------------------

async fn test_segment_round_trip<S: ProjectRepository + SegmentRepository>(store: &S) {
    let proj = make_project("seg-proj");
    store.upsert_project(&proj).await.unwrap();

    let seg = make_segment("beta-users");
    store.upsert_segment(&proj.key, &seg).await.unwrap();

    let fetched = store
        .get_segment(&proj.key, &seg.key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.name, seg.name);
    assert_eq!(
        fetched.match_expr, seg.match_expr,
        "recursive SegmentMatch tree must round-trip faithfully"
    );

    store.delete_project(&proj.key).await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 8: flag_env_config_round_trip
// ---------------------------------------------------------------------------

async fn test_flag_env_config_round_trip<
    S: ProjectRepository + EnvironmentRepository + FlagRepository + FlagEnvConfigRepository,
>(
    store: &S,
) {
    let proj = make_project("fec-proj");
    let env = make_env("prod");
    let flag = make_flag("my-feature");
    let config = make_flag_env_config();

    store.upsert_project(&proj).await.unwrap();
    store.upsert_environment(&proj.key, &env).await.unwrap();
    store.upsert_flag(&proj.key, &flag).await.unwrap();
    store
        .upsert_flag_env_config(&proj.key, &flag.key, &env.key, &config)
        .await
        .unwrap();

    let fetched = store
        .get_flag_env_config(&proj.key, &flag.key, &env.key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.enabled, config.enabled);
    assert_eq!(fetched.rules, config.rules);
    assert_eq!(fetched.default_rule, config.default_rule);

    store.delete_project(&proj.key).await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 9: cascade_delete
// ---------------------------------------------------------------------------

async fn test_cascade_delete<
    S: ProjectRepository
        + EnvironmentRepository
        + FlagRepository
        + SegmentRepository
        + FlagEnvConfigRepository
        + SdkKeyRepository,
>(
    store: &S,
) {
    let proj = make_project("cascade-proj");
    let env = make_env("prod");
    let flag = make_flag("killswitch");
    let seg = make_segment("all-users");
    let config = make_flag_env_config();

    store.upsert_project(&proj).await.unwrap();
    store.upsert_environment(&proj.key, &env).await.unwrap();
    store.upsert_flag(&proj.key, &flag).await.unwrap();
    store.upsert_segment(&proj.key, &seg).await.unwrap();
    store
        .upsert_flag_env_config(&proj.key, &flag.key, &env.key, &config)
        .await
        .unwrap();

    let sdk_new = NewSdkKey {
        kind: SdkKeyKind::Server,
        scope: SdkKeyScope {
            project_key: proj.key.clone(),
            environment_key: env.key.clone(),
        },
    };
    store
        .create_sdk_key("cascade-raw-sdk-key-12345", &sdk_new)
        .await
        .unwrap();

    store.delete_project(&proj.key).await.unwrap();

    assert!(
        store.get_project(&proj.key).await.unwrap().is_none(),
        "project must be gone"
    );
    assert!(
        store
            .get_environment(&proj.key, &env.key)
            .await
            .unwrap()
            .is_none(),
        "environment must be cascade-deleted"
    );
    assert!(
        store
            .get_flag(&proj.key, &flag.key)
            .await
            .unwrap()
            .is_none(),
        "flag must be cascade-deleted"
    );
    assert!(
        store
            .get_segment(&proj.key, &seg.key)
            .await
            .unwrap()
            .is_none(),
        "segment must be cascade-deleted"
    );
    assert!(
        store
            .get_flag_env_config(&proj.key, &flag.key, &env.key)
            .await
            .unwrap()
            .is_none(),
        "flag_env_config must be cascade-deleted"
    );
    assert!(
        store
            .find_sdk_key("cascade-raw-sdk-key-12345")
            .await
            .unwrap()
            .is_none(),
        "sdk_key must be cascade-deleted"
    );
}

// ---------------------------------------------------------------------------
// Test 11: sdk_key_lookup_by_raw
// ---------------------------------------------------------------------------

async fn test_sdk_key_lookup_by_raw<
    S: ProjectRepository + EnvironmentRepository + SdkKeyRepository,
>(
    store: &S,
) {
    let proj = make_project("sdk-proj");
    let env = make_env("staging");
    store.upsert_project(&proj).await.unwrap();
    store.upsert_environment(&proj.key, &env).await.unwrap();

    let new_key = NewSdkKey {
        kind: SdkKeyKind::Client,
        scope: SdkKeyScope {
            project_key: proj.key.clone(),
            environment_key: env.key.clone(),
        },
    };
    let raw = "sdk-client-raw-key-xyz";
    store.create_sdk_key(raw, &new_key).await.unwrap();

    let found = store.find_sdk_key(raw).await.unwrap();
    assert!(found.is_some(), "should find the sdk key by raw value");
    let record = found.unwrap();
    assert_eq!(record.kind, SdkKeyKind::Client);
    assert_eq!(record.scope.project_key, proj.key);

    let not_found = store.find_sdk_key("nonexistent-key").await.unwrap();
    assert!(not_found.is_none(), "unknown raw key must return None");

    store.delete_project(&proj.key).await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 12: sdk_key_pepper_changes_hash (no I/O needed)
// ---------------------------------------------------------------------------

fn test_sdk_key_pepper_changes_hash() {
    let hasher_a = KeyHasher::new(b"pepper-alpha".to_vec());
    let hasher_b = KeyHasher::new(b"pepper-beta".to_vec());
    let raw = "same-raw-key";
    assert_ne!(
        hasher_a.hash(raw),
        hasher_b.hash(raw),
        "different peppers must produce different hashes"
    );
}

// ---------------------------------------------------------------------------
// Test 13: transaction_commit_persists
// ---------------------------------------------------------------------------

async fn test_transaction_commit_persists<S>(store: &S)
where
    S: ProjectRepository + FlagRepository + TransactionalStore + 'static,
    for<'a> <S as TransactionalStore>::Session<'a>: WriteSession,
{
    let proj = make_project("tx-commit-proj");
    let flag = make_flag("tx-flag");

    let mut session = store.begin().await.unwrap();
    session.upsert_project(&proj).await.unwrap();
    session.upsert_flag(&proj.key, &flag).await.unwrap();
    session.commit().await.unwrap();

    let fetched_proj = store.get_project(&proj.key).await.unwrap();
    assert!(
        fetched_proj.is_some(),
        "committed project must be visible after commit"
    );
    let fetched_flag = store.get_flag(&proj.key, &flag.key).await.unwrap();
    assert!(
        fetched_flag.is_some(),
        "committed flag must be visible after commit"
    );

    store.delete_project(&proj.key).await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 14: transaction_drop_rolls_back
// ---------------------------------------------------------------------------

async fn test_transaction_drop_rolls_back<S>(store: &S)
where
    S: ProjectRepository + TransactionalStore + 'static,
    for<'a> <S as TransactionalStore>::Session<'a>: WriteSession,
{
    let proj = make_project("tx-rollback-proj");

    {
        let mut session = store.begin().await.unwrap();
        session.upsert_project(&proj).await.unwrap();
        // session dropped here without commit -> rollback
    }

    let fetched = store.get_project(&proj.key).await.unwrap();
    assert!(fetched.is_none(), "rolled-back project must not be visible");
}
