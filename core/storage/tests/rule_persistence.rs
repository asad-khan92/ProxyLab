//! Rules, stored and read back.
//!
//! Every test gets a database of its own in a temporary directory, so
//! nothing here touches a developer's own rules or traffic.

mod support;

use support::TestStore;

use proxy_storage::StorageError;
use rules::{
    BodyEdit, HeaderEdit, MapLocal, ModifyRequest, ModifyResponse, NewRule, PathEdit, QueryEdit,
    Rule, RuleAction, RuleId, RuleMatcher, RuleUpdate, MAX_RULE_NAME_LENGTH,
};

/// A rule called `name` at `priority`, blocking everything on one host.
fn rule(name: &str, host: &str, priority: i32) -> Rule {
    Rule::create(
        NewRule::new(
            name,
            RuleMatcher::any().with_host(host),
            RuleAction::block(),
        )
        .with_priority(priority),
    )
    .expect("a valid rule")
}

/// The ids of a list, in the order the list holds them.
fn ids(rules: &[Rule]) -> Vec<RuleId> {
    rules.iter().map(|rule| rule.id).collect()
}

#[test]
fn a_rule_is_read_back_exactly_as_it_was_stored() {
    let store = TestStore::open();
    let stored = rule("block the api", "api.example.com", 7);

    store.storage.insert_rule(&stored).expect("stores");

    let read = store
        .storage
        .rule(stored.id)
        .expect("reads")
        .expect("the rule is stored");

    assert_eq!(read, stored);
}

#[test]
fn every_action_survives_being_stored() {
    let store = TestStore::open();
    let actions = [
        RuleAction::Allow,
        RuleAction::block(),
        RuleAction::block_with(451),
        RuleAction::MapLocal(
            MapLocal::new("/tmp/fixture.json").with_content_type("application/json"),
        ),
        RuleAction::map_remote("https://staging.example.com:8443/v2?fixed=yes")
            .expect("a valid destination"),
        RuleAction::redirect("https://new.example.com/moved", 308).expect("a valid redirect"),
        RuleAction::ModifyRequest(ModifyRequest {
            method: Some("PUT".into()),
            path: Some(PathEdit::Set {
                path: "/v2/users".into(),
            }),
            query: vec![QueryEdit::Remove {
                name: "debug".into(),
            }],
            headers: vec![HeaderEdit::Add {
                name: "X-Trace".into(),
                value: "1".into(),
            }],
            body: Some(BodyEdit::SetJson {
                path: "a.b".into(),
                value: "[1,2]".into(),
            }),
        }),
        RuleAction::ModifyResponse(ModifyResponse {
            status: Some(503),
            headers: vec![HeaderEdit::Remove {
                name: "Set-Cookie".into(),
            }],
            body: Some(BodyEdit::Replace {
                text: "{\"down\":true}".into(),
            }),
        }),
    ];

    for action in actions {
        let stored = Rule::create(NewRule::new("rule", RuleMatcher::any(), action.clone()))
            .expect("a valid rule");
        store.storage.insert_rule(&stored).expect("stores");

        let read = store
            .storage
            .rule(stored.id)
            .expect("reads")
            .expect("the rule is stored");

        assert_eq!(read.action, action);
    }
}

#[test]
fn every_matcher_survives_being_stored() {
    let store = TestStore::open();
    let matchers = [
        RuleMatcher::any(),
        RuleMatcher::any().with_host("api.example.com"),
        RuleMatcher::any().with_path("/users"),
        RuleMatcher::any().with_path_prefix("/assets/"),
        RuleMatcher::any().with_method("POST"),
        RuleMatcher::any()
            .with_host("api.example.com")
            .with_path("/users")
            .with_method("GET"),
    ];

    for matcher in matchers {
        let stored = Rule::create(NewRule::new("rule", matcher.clone(), RuleAction::Allow))
            .expect("a valid rule");
        store.storage.insert_rule(&stored).expect("stores");

        assert_eq!(
            store
                .storage
                .rule(stored.id)
                .expect("reads")
                .expect("stored")
                .matcher,
            matcher
        );
    }
}

#[test]
fn a_rule_that_was_never_stored_is_not_found() {
    let store = TestStore::open();

    assert_eq!(store.storage.rule(RuleId::next()).expect("reads"), None);
}

#[test]
fn storing_a_rule_twice_is_refused_and_keeps_the_first() {
    let store = TestStore::open();
    let first = rule("first", "api.example.com", 0);
    store.storage.insert_rule(&first).expect("stores");

    let mut clash = rule("second", "other.example.com", 5);
    clash.id = first.id;

    assert!(store.storage.insert_rule(&clash).is_err());
    assert_eq!(
        store
            .storage
            .rule(first.id)
            .expect("reads")
            .expect("stored"),
        first
    );
}

#[test]
fn rules_are_listed_in_the_order_they_are_consulted_in() {
    let store = TestStore::open();
    let late = rule("late", "a.example.com", 20);
    let early = rule("early", "b.example.com", -5);
    let middle = rule("middle", "c.example.com", 0);

    // Stored in an order that is none of the orders that matter.
    for stored in [&middle, &late, &early] {
        store.storage.insert_rule(stored).expect("stores");
    }

    assert_eq!(
        ids(&store.storage.list_rules().expect("lists")),
        vec![early.id, middle.id, late.id]
    );
}

#[test]
fn rules_of_equal_priority_are_listed_oldest_first() {
    let store = TestStore::open();
    let first = rule("first", "a.example.com", 10);
    let second = rule("second", "b.example.com", 10);
    let third = rule("third", "c.example.com", 10);
    assert!(
        first.id < second.id && second.id < third.id,
        "ids are ordered"
    );

    for stored in [&third, &first, &second] {
        store.storage.insert_rule(stored).expect("stores");
    }

    assert_eq!(
        ids(&store.storage.list_rules().expect("lists")),
        vec![first.id, second.id, third.id]
    );
}

#[test]
fn listing_an_empty_database_is_an_empty_list() {
    let store = TestStore::open();

    assert!(store.storage.list_rules().expect("lists").is_empty());
    assert_eq!(store.storage.count_rules().expect("counts"), 0);
}

#[test]
fn rules_outlive_the_process_that_stored_them() {
    let store = TestStore::open();
    let stored = [
        rule("one", "a.example.com", 3),
        rule("two", "b.example.com", 1),
        rule("three", "c.example.com", 2),
    ];
    for rule in &stored {
        store.storage.insert_rule(rule).expect("stores");
    }

    // A different handle to the same files, as a restart produces.
    let reopened = store.reopen();

    let listed = reopened.list_rules().expect("lists");
    assert_eq!(listed.len(), 3);
    // And in the same order, which is what makes the decisions the same.
    assert_eq!(
        listed.iter().map(|rule| rule.priority).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        reopened.rule(stored[0].id).expect("reads").expect("stored"),
        stored[0]
    );
}

#[test]
fn an_update_changes_what_it_names_and_nothing_else() {
    let store = TestStore::open();
    let original = rule("original", "api.example.com", 10);
    store.storage.insert_rule(&original).expect("stores");

    let updated = store
        .storage
        .update_rule(
            original.id,
            RuleUpdate {
                name: Some("renamed".to_owned()),
                priority: Some(3),
                ..RuleUpdate::none()
            },
        )
        .expect("updates")
        .expect("the rule is stored");

    assert_eq!(updated.name, "renamed");
    assert_eq!(updated.priority, 3);
    // Untouched.
    assert_eq!(updated.id, original.id);
    assert_eq!(updated.matcher, original.matcher);
    assert_eq!(updated.action, original.action);
    assert_eq!(updated.enabled, original.enabled);
    assert_eq!(updated.created_at_ms, original.created_at_ms);
    assert!(updated.updated_at_ms >= original.updated_at_ms);

    // And it is what a later read gives back.
    assert_eq!(
        store
            .storage
            .rule(original.id)
            .expect("reads")
            .expect("stored"),
        updated
    );
}

#[test]
fn an_update_can_replace_the_matcher_and_the_action() {
    let store = TestStore::open();
    let original = rule("original", "api.example.com", 0);
    store.storage.insert_rule(&original).expect("stores");

    let matcher = RuleMatcher::any().with_path_prefix("/assets/");
    let action = RuleAction::map_remote("http://127.0.0.1:8080").expect("a valid destination");
    let updated = store
        .storage
        .update_rule(
            original.id,
            RuleUpdate {
                matcher: Some(matcher.clone()),
                action: Some(action.clone()),
                ..RuleUpdate::none()
            },
        )
        .expect("updates")
        .expect("stored");

    assert_eq!(updated.matcher, matcher);
    assert_eq!(updated.action, action);
}

#[test]
fn a_rule_can_be_turned_off_and_on_again() {
    let store = TestStore::open();
    let stored = rule("rule", "api.example.com", 0);
    store.storage.insert_rule(&stored).expect("stores");
    assert!(stored.enabled);

    let disabled = store
        .storage
        .update_rule(stored.id, RuleUpdate::enabled(false))
        .expect("updates")
        .expect("stored");
    assert!(!disabled.enabled);
    assert!(
        !store
            .reopen()
            .rule(stored.id)
            .expect("reads")
            .expect("stored")
            .enabled
    );

    let enabled = store
        .storage
        .update_rule(stored.id, RuleUpdate::enabled(true))
        .expect("updates")
        .expect("stored");
    assert!(enabled.enabled);
}

#[test]
fn updating_a_rule_that_is_not_there_says_so() {
    let store = TestStore::open();

    assert_eq!(
        store
            .storage
            .update_rule(RuleId::next(), RuleUpdate::enabled(false))
            .expect("updates"),
        None
    );
}

#[test]
fn an_update_that_would_make_a_rule_invalid_is_refused() {
    let store = TestStore::open();
    let stored = rule("rule", "api.example.com", 0);
    store.storage.insert_rule(&stored).expect("stores");

    let refused = store.storage.update_rule(
        stored.id,
        RuleUpdate {
            name: Some(String::new()),
            ..RuleUpdate::none()
        },
    );

    assert!(refused.is_err());
    // And the rule that is there is the one that was there.
    assert_eq!(
        store
            .storage
            .rule(stored.id)
            .expect("reads")
            .expect("stored"),
        stored
    );
}

#[test]
fn deleting_a_rule_removes_it() {
    let store = TestStore::open();
    let stored = rule("rule", "api.example.com", 0);
    store.storage.insert_rule(&stored).expect("stores");

    assert!(store.storage.delete_rule(stored.id).expect("deletes"));
    assert_eq!(store.storage.rule(stored.id).expect("reads"), None);
    assert!(store.storage.list_rules().expect("lists").is_empty());
    // And it stays deleted.
    assert!(store.reopen().list_rules().expect("lists").is_empty());
}

#[test]
fn deleting_a_rule_that_is_not_there_says_so() {
    let store = TestStore::open();

    assert!(!store.storage.delete_rule(RuleId::next()).expect("deletes"));
}

#[test]
fn deleting_one_rule_leaves_the_others() {
    let store = TestStore::open();
    let kept = rule("kept", "a.example.com", 0);
    let removed = rule("removed", "b.example.com", 1);
    store.storage.insert_rule(&kept).expect("stores");
    store.storage.insert_rule(&removed).expect("stores");

    assert!(store.storage.delete_rule(removed.id).expect("deletes"));

    assert_eq!(
        ids(&store.storage.list_rules().expect("lists")),
        vec![kept.id]
    );
}

#[test]
fn clearing_removes_every_rule_and_says_how_many() {
    let store = TestStore::open();
    for index in 0..4 {
        store
            .storage
            .insert_rule(&rule("rule", &format!("{index}.example.com"), index))
            .expect("stores");
    }

    assert_eq!(store.storage.clear_rules().expect("clears"), 4);
    assert!(store.storage.list_rules().expect("lists").is_empty());
    // And the table is still usable afterwards.
    let after = rule("after", "after.example.com", 0);
    store.storage.insert_rule(&after).expect("stores");
    assert_eq!(store.storage.count_rules().expect("counts"), 1);
}

#[test]
fn a_rule_row_this_build_cannot_read_is_reported_rather_than_guessed_at() {
    let store = TestStore::open();
    let stored = rule("rule", "api.example.com", 0);
    store.storage.insert_rule(&stored).expect("stores");

    // As though a later ProxyLab had written an action this one has never
    // heard of, or somebody had edited the row by hand.
    store.execute(
        "UPDATE rules SET action = '{\"type\":\"rewrite_headers\"}' WHERE id = ?1",
        &stored.id.as_text(),
    );

    assert!(matches!(
        store.storage.rule(stored.id),
        Err(StorageError::Unreadable { .. })
    ));
    assert!(matches!(
        store.storage.list_rules(),
        Err(StorageError::Unreadable { .. })
    ));
}

#[test]
fn a_rule_row_whose_matcher_is_not_json_is_reported() {
    let store = TestStore::open();
    let stored = rule("rule", "api.example.com", 0);
    store.storage.insert_rule(&stored).expect("stores");

    store.execute(
        "UPDATE rules SET matcher = 'not json' WHERE id = ?1",
        &stored.id.as_text(),
    );

    assert!(matches!(
        store.storage.rule(stored.id),
        Err(StorageError::Unreadable { .. })
    ));
}

#[test]
fn a_rule_row_that_would_not_be_a_valid_rule_is_reported() {
    let store = TestStore::open();
    let stored = rule("rule", "api.example.com", 0);
    store.storage.insert_rule(&stored).expect("stores");

    // A matcher that parses and could never match anything.
    store.execute(
        "UPDATE rules SET matcher = '{\"path\":{\"exact\":\"no-slash\"}}' WHERE id = ?1",
        &stored.id.as_text(),
    );

    assert!(matches!(
        store.storage.rule(stored.id),
        Err(StorageError::Unreadable { .. })
    ));
}

#[test]
fn a_rule_name_of_the_greatest_length_allowed_survives_the_round_trip() {
    let store = TestStore::open();
    let name = "x".repeat(MAX_RULE_NAME_LENGTH);
    let stored = Rule::create(NewRule::new(
        name.clone(),
        RuleMatcher::any(),
        RuleAction::Allow,
    ))
    .expect("a valid rule");

    store.storage.insert_rule(&stored).expect("stores");

    assert_eq!(
        store
            .storage
            .rule(stored.id)
            .expect("reads")
            .expect("stored")
            .name,
        name
    );
}

#[test]
fn priorities_at_the_edges_survive_the_round_trip() {
    let store = TestStore::open();

    for priority in [i32::MIN, -1, 0, 1, i32::MAX] {
        let stored = Rule::create(
            NewRule::new("rule", RuleMatcher::any(), RuleAction::Allow).with_priority(priority),
        )
        .expect("a valid rule");
        store.storage.insert_rule(&stored).expect("stores");

        assert_eq!(
            store
                .storage
                .rule(stored.id)
                .expect("reads")
                .expect("stored")
                .priority,
            priority
        );
    }
}

#[test]
fn rules_and_traffic_do_not_disturb_each_other() {
    // The rules table was added beside the traffic tables, not into them.
    let store = TestStore::open();
    let exchange = support::http_exchange("api.example.com", "/users", 200);
    store.storage.insert(&exchange).expect("stores traffic");
    let stored = rule("rule", "api.example.com", 0);
    store.storage.insert_rule(&stored).expect("stores a rule");

    assert_eq!(store.storage.count().expect("counts traffic"), 1);
    assert_eq!(store.storage.count_rules().expect("counts rules"), 1);

    store.storage.clear().expect("clears traffic");
    assert_eq!(store.storage.count().expect("counts traffic"), 0);
    assert_eq!(store.storage.count_rules().expect("counts rules"), 1);

    store.storage.clear_rules().expect("clears rules");
    assert_eq!(store.storage.count_rules().expect("counts rules"), 0);
}
