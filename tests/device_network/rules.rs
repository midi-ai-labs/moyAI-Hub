use super::*;

#[test]
fn rule_scope_is_unique_across_conflicting_values_but_direction_usage_and_profile_are_distinct() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    f.allow(&a, &b, Usage::Direct);
    let original: Rule =
        serde_json::from_value(f.owner.snapshot(f.now)["rules"][0].clone()).unwrap();
    for effect in [Effect::Allow, Effect::Deny] {
        let mut duplicate = original.clone();
        duplicate.id.clear();
        duplicate.effect = effect;
        duplicate.max_depth = 1;
        let revision = f.owner.revision();
        assert!(matches!(
            f.owner.set_rule(&revision, duplicate),
            Err(NetworkError::ConnectionRuleConflict)
        ));
        assert_eq!(f.owner.revision(), revision);
    }
    let mut update = original.clone();
    update.effect = Effect::Deny;
    f.owner.set_rule(&f.owner.revision(), update).unwrap();
    for variant in 0..3 {
        let mut independent = original.clone();
        independent.id.clear();
        match variant {
            0 => std::mem::swap(&mut independent.from, &mut independent.to),
            1 => independent.usage = Usage::Delegated,
            _ => independent.profile_id = None,
        }
        f.owner.set_rule(&f.owner.revision(), independent).unwrap();
    }
    let reopened =
        DeviceNetwork::open(&f.directory.path().join("network.json"), "hub-test").unwrap();
    assert_eq!(
        reopened.snapshot(f.now)["rules"].as_array().unwrap().len(),
        4
    );
}

#[test]
fn legacy_duplicates_are_retained_until_explicit_deletion_and_cannot_be_extended() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    f.allow(&a, &b, Usage::Direct);
    let path = f.directory.path().join("network.json");
    let mut persisted: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut duplicate = persisted["rules"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    duplicate["id"] = json!("legacy-duplicate");
    duplicate["effect"] = json!("deny");
    persisted["rules"]["legacy-duplicate"] = duplicate;
    std::fs::write(&path, serde_json::to_vec(&persisted).unwrap()).unwrap();
    let mut reopened = DeviceNetwork::open(&path, "hub-test").unwrap();
    assert_eq!(
        reopened.snapshot(f.now)["rules"].as_array().unwrap().len(),
        2
    );
    let rule: Rule = serde_json::from_value(reopened.snapshot(f.now)["rules"][0].clone()).unwrap();
    assert!(matches!(
        reopened.set_rule(&reopened.revision(), rule.clone()),
        Err(NetworkError::ConnectionRuleConflict)
    ));
    reopened
        .delete_rule(&reopened.revision(), "legacy-duplicate")
        .unwrap();
    let remaining: Rule =
        serde_json::from_value(reopened.snapshot(f.now)["rules"][0].clone()).unwrap();
    reopened.set_rule(&reopened.revision(), remaining).unwrap();
}
