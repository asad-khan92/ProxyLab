//! What a rule set decides, and why.
//!
//! The whole of the matching surface, positive and negative, plus the
//! ordering that decides between rules that all match. Every test here runs
//! without a proxy, a database, a socket, or a file — which is the point of
//! the engine being pure.

use rules::{
    BlockAction, MapLocal, NewRule, RemoteTarget, RequestContext, Rule, RuleAction, RuleDecision,
    RuleId, RuleMatcher, RuleSet,
};

/// A rule called `name` that does `action` to what `matcher` describes.
fn rule(name: &str, matcher: RuleMatcher, action: RuleAction, priority: i32) -> Rule {
    Rule::create(NewRule::new(name, matcher, action).with_priority(priority)).expect("a valid rule")
}

/// A rule that blocks what `matcher` describes.
fn blocking(matcher: RuleMatcher, priority: i32) -> Rule {
    rule("block", matcher, RuleAction::block(), priority)
}

/// A rule that allows what `matcher` describes.
fn allowing(matcher: RuleMatcher, priority: i32) -> Rule {
    rule("allow", matcher, RuleAction::Allow, priority)
}

/// The request `method` to `path` on `host`.
fn request<'a>(method: &'a str, host: &'a str, path: &'a str) -> RequestContext<'a> {
    RequestContext::new(host, path, method)
}

/// The matcher from the example in the step: one host, one path, one method.
fn users_matcher() -> RuleMatcher {
    RuleMatcher::any()
        .with_host("api.example.com")
        .with_path("/users")
        .with_method("GET")
}

// MARK: - Nothing to decide

#[test]
fn no_rules_allow_everything() {
    let rules = RuleSet::empty();

    assert_eq!(
        rules.evaluate(&request("GET", "api.example.com", "/users")),
        RuleDecision::allowed()
    );
    assert!(rules.is_empty());
}

#[test]
fn a_rule_that_does_not_match_leaves_the_request_alone() {
    let rules = RuleSet::new([blocking(
        RuleMatcher::any().with_host("other.example.com"),
        0,
    )]);

    assert_eq!(
        rules.evaluate(&request("GET", "api.example.com", "/users")),
        RuleDecision::allowed()
    );
}

#[test]
fn a_disabled_rule_is_never_consulted() {
    let mut rule = blocking(RuleMatcher::any().with_host("api.example.com"), 0);
    rule.enabled = false;
    let rules = RuleSet::new([rule]);

    assert_eq!(
        rules.evaluate(&request("GET", "api.example.com", "/users")),
        RuleDecision::allowed()
    );
    assert_eq!(rules.len(), 1);
    assert_eq!(rules.enabled_count(), 0);
}

#[test]
fn a_disabled_rule_lets_a_later_one_decide() {
    let mut disabled = blocking(RuleMatcher::any().with_host("api.example.com"), 0);
    disabled.enabled = false;
    let mapped = rule(
        "map",
        RuleMatcher::any().with_host("api.example.com"),
        RuleAction::map_remote("https://staging.example.com").expect("a valid destination"),
        10,
    );
    let rules = RuleSet::new([disabled, mapped.clone()]);

    assert_eq!(
        rules
            .evaluate(&request("GET", "api.example.com", "/users"))
            .rule(),
        Some(mapped.id)
    );
}

// MARK: - One criterion at a time

#[test]
fn a_host_matcher_matches_that_host_and_no_other() {
    let rules = RuleSet::new([blocking(RuleMatcher::any().with_host("api.example.com"), 0)]);

    assert!(!rules
        .evaluate(&request("GET", "api.example.com", "/anything"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "other.example.com", "/anything"))
        .is_allow());
    // A subdomain is a different host, and a suffix is not a match.
    assert!(rules
        .evaluate(&request("GET", "sub.api.example.com", "/x"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "example.com", "/x"))
        .is_allow());
}

#[test]
fn a_host_matcher_does_not_care_how_the_host_was_spelled() {
    let rules = RuleSet::new([blocking(RuleMatcher::any().with_host("API.Example.COM"), 0)]);

    assert!(!rules
        .evaluate(&request("GET", "api.example.com", "/x"))
        .is_allow());
}

#[test]
fn a_path_matcher_matches_that_path_exactly() {
    let rules = RuleSet::new([blocking(RuleMatcher::any().with_path("/users"), 0)]);

    assert!(!rules
        .evaluate(&request("GET", "api.example.com", "/users"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "api.example.com", "/users/1"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "api.example.com", "/orders"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "api.example.com", "/"))
        .is_allow());
}

#[test]
fn a_path_matcher_cares_about_case() {
    // A URL path is case-sensitive, unlike a host.
    let rules = RuleSet::new([blocking(RuleMatcher::any().with_path("/Users"), 0)]);

    assert!(!rules
        .evaluate(&request("GET", "api.example.com", "/Users"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "api.example.com", "/users"))
        .is_allow());
}

#[test]
fn a_path_prefix_matcher_matches_everything_under_it() {
    let rules = RuleSet::new([blocking(RuleMatcher::any().with_path_prefix("/assets/"), 0)]);

    assert!(!rules
        .evaluate(&request("GET", "example.com", "/assets/"))
        .is_allow());
    assert!(!rules
        .evaluate(&request("GET", "example.com", "/assets/logo.png"))
        .is_allow());
    assert!(!rules
        .evaluate(&request("GET", "example.com", "/assets/js/app.js"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "example.com", "/assets"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "example.com", "/other"))
        .is_allow());
}

#[test]
fn a_path_prefix_is_a_string_prefix_and_says_so() {
    // `/assets` without the slash matches `/assetsX` too. That is what a
    // string prefix is, and a rule that means the directory writes the slash.
    let rules = RuleSet::new([blocking(RuleMatcher::any().with_path_prefix("/assets"), 0)]);

    assert!(!rules
        .evaluate(&request("GET", "example.com", "/assetsX"))
        .is_allow());
    assert!(!rules
        .evaluate(&request("GET", "example.com", "/assets/logo.png"))
        .is_allow());
}

#[test]
fn a_method_matcher_matches_that_method_and_no_other() {
    let rules = RuleSet::new([blocking(RuleMatcher::any().with_method("GET"), 0)]);

    assert!(!rules
        .evaluate(&request("GET", "example.com", "/x"))
        .is_allow());
    assert!(rules
        .evaluate(&request("POST", "example.com", "/x"))
        .is_allow());
    assert!(rules
        .evaluate(&request("DELETE", "example.com", "/x"))
        .is_allow());
}

#[test]
fn a_method_matcher_does_not_care_how_the_method_was_spelled() {
    // Every method that travels is uppercase, so a rule written `get` is a
    // typo and not an intent to match nothing.
    let rules = RuleSet::new([blocking(RuleMatcher::any().with_method("get"), 0)]);

    assert!(!rules
        .evaluate(&request("GET", "example.com", "/x"))
        .is_allow());
}

// MARK: - Criteria in combination

#[test]
fn host_and_path_must_both_match() {
    let matcher = RuleMatcher::any()
        .with_host("api.example.com")
        .with_path("/users");
    let rules = RuleSet::new([blocking(matcher, 0)]);

    assert!(!rules
        .evaluate(&request("GET", "api.example.com", "/users"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "api.example.com", "/orders"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "cdn.example.com", "/users"))
        .is_allow());
}

#[test]
fn host_and_method_must_both_match() {
    let matcher = RuleMatcher::any()
        .with_host("api.example.com")
        .with_method("POST");
    let rules = RuleSet::new([blocking(matcher, 0)]);

    assert!(!rules
        .evaluate(&request("POST", "api.example.com", "/anything"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "api.example.com", "/anything"))
        .is_allow());
    assert!(rules
        .evaluate(&request("POST", "cdn.example.com", "/anything"))
        .is_allow());
}

#[test]
fn path_and_method_must_both_match() {
    let matcher = RuleMatcher::any().with_path("/users").with_method("GET");
    let rules = RuleSet::new([blocking(matcher, 0)]);

    assert!(!rules
        .evaluate(&request("GET", "anywhere.example.com", "/users"))
        .is_allow());
    assert!(rules
        .evaluate(&request("POST", "anywhere.example.com", "/users"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "anywhere.example.com", "/orders"))
        .is_allow());
}

#[test]
fn all_three_criteria_must_match() {
    // The example from the step, in full.
    let rules = RuleSet::new([blocking(users_matcher(), 0)]);

    assert!(!rules
        .evaluate(&request("GET", "api.example.com", "/users"))
        .is_allow());
    assert!(rules
        .evaluate(&request("POST", "api.example.com", "/users"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "other.example.com", "/users"))
        .is_allow());
    assert!(rules
        .evaluate(&request("GET", "api.example.com", "/orders"))
        .is_allow());
}

#[test]
fn a_matcher_naming_nothing_matches_every_request() {
    // Which is how "block everything except these" is written.
    let rules = RuleSet::new([blocking(RuleMatcher::any(), 0)]);

    for (method, host, path) in [
        ("GET", "example.com", "/"),
        ("POST", "api.example.com", "/users"),
        ("DELETE", "cdn.example.com", "/assets/logo.png"),
    ] {
        assert!(!rules.evaluate(&request(method, host, path)).is_allow());
    }
}

// MARK: - Which rule wins

#[test]
fn the_lower_priority_number_wins() {
    // The example from the step: a block over the host, and an allow for one
    // path in front of it.
    let block = blocking(RuleMatcher::any().with_host("api.example.com"), 10);
    let allow = allowing(
        RuleMatcher::any()
            .with_host("api.example.com")
            .with_path("/health"),
        5,
    );
    let rules = RuleSet::new([block.clone(), allow.clone()]);

    assert_eq!(
        rules.evaluate(&request("GET", "api.example.com", "/health")),
        RuleDecision::Allow {
            rule: Some(allow.id)
        }
    );
    assert_eq!(
        rules
            .evaluate(&request("GET", "api.example.com", "/users"))
            .rule(),
        Some(block.id)
    );
}

#[test]
fn the_order_rules_are_given_in_does_not_decide_anything() {
    let block = blocking(RuleMatcher::any().with_host("api.example.com"), 10);
    let allow = allowing(
        RuleMatcher::any()
            .with_host("api.example.com")
            .with_path("/health"),
        5,
    );

    let one_way = RuleSet::new([block.clone(), allow.clone()]);
    let other_way = RuleSet::new([allow.clone(), block.clone()]);
    let health = request("GET", "api.example.com", "/health");

    assert_eq!(one_way.evaluate(&health), other_way.evaluate(&health));
    assert_eq!(one_way.rules(), other_way.rules());
}

#[test]
fn equal_priorities_are_broken_by_which_rule_was_created_first() {
    let first = blocking(RuleMatcher::any().with_host("api.example.com"), 10);
    let second = allowing(RuleMatcher::any().with_host("api.example.com"), 10);
    assert!(first.id < second.id, "ids are minted in order");

    let rules = RuleSet::new([second.clone(), first.clone()]);

    assert_eq!(
        rules
            .evaluate(&request("GET", "api.example.com", "/x"))
            .rule(),
        Some(first.id)
    );
}

#[test]
fn a_negative_priority_is_consulted_before_a_zero_one() {
    let default = blocking(RuleMatcher::any(), 0);
    let first = allowing(RuleMatcher::any().with_path("/health"), -10);
    let rules = RuleSet::new([default, first.clone()]);

    assert_eq!(
        rules
            .evaluate(&request("GET", "example.com", "/health"))
            .rule(),
        Some(first.id)
    );
}

#[test]
fn only_the_first_matching_rule_decides() {
    // Three rules that all match; the first is the only one consulted, so
    // the decision is its action and not a combination of the three.
    let first = rule(
        "first",
        RuleMatcher::any(),
        RuleAction::map_remote("https://staging.example.com").expect("a valid destination"),
        1,
    );
    let second = blocking(RuleMatcher::any(), 2);
    let third = rule(
        "third",
        RuleMatcher::any(),
        RuleAction::map_local("/tmp/x"),
        3,
    );
    let rules = RuleSet::new([first.clone(), second, third]);

    let decision = rules.evaluate(&request("GET", "example.com", "/x"));

    assert_eq!(decision.rule(), Some(first.id));
    assert_eq!(decision.name(), "map_remote");
}

#[test]
fn the_same_rules_always_decide_the_same_way() {
    let rules: Vec<Rule> = (0..32)
        .map(|index| blocking(RuleMatcher::any().with_path(format!("/{index}")), index % 4))
        .collect();

    let built_once = RuleSet::new(rules.clone());
    let mut shuffled = rules.clone();
    shuffled.reverse();
    let built_again = RuleSet::new(shuffled);

    assert_eq!(built_once.rules(), built_again.rules());
    for index in 0..32 {
        let path = format!("/{index}");
        let request = request("GET", "example.com", &path);

        assert_eq!(
            built_once.evaluate(&request),
            built_again.evaluate(&request)
        );
    }
}

// MARK: - What a decision carries

#[test]
fn an_allow_rule_produces_an_allow_naming_it() {
    let allow = allowing(RuleMatcher::any().with_host("example.com"), 0);
    let rules = RuleSet::new([allow.clone()]);

    assert_eq!(
        rules.evaluate(&request("GET", "example.com", "/x")),
        RuleDecision::Allow {
            rule: Some(allow.id)
        }
    );
}

#[test]
fn no_rule_at_all_produces_an_allow_naming_none() {
    assert_eq!(
        RuleSet::empty().evaluate(&request("GET", "example.com", "/x")),
        RuleDecision::Allow { rule: None }
    );
    assert_eq!(
        RuleSet::empty()
            .evaluate(&request("GET", "example.com", "/x"))
            .rule(),
        None
    );
}

#[test]
fn a_block_carries_the_status_to_answer_with() {
    let rules = RuleSet::new([rule(
        "teapot",
        RuleMatcher::any(),
        RuleAction::block_with(418),
        0,
    )]);

    match rules.evaluate(&request("GET", "example.com", "/x")) {
        RuleDecision::Block { block, .. } => assert_eq!(block.status, 418),
        other => panic!("expected a block, got {other:?}"),
    }
}

#[test]
fn a_block_with_no_status_chosen_is_a_403() {
    assert_eq!(BlockAction::default().status, 403);

    let rules = RuleSet::new([blocking(RuleMatcher::any(), 0)]);

    match rules.evaluate(&request("GET", "example.com", "/x")) {
        RuleDecision::Block { block, .. } => assert_eq!(block.status, 403),
        other => panic!("expected a block, got {other:?}"),
    }
}

#[test]
fn a_map_local_carries_the_file_and_opens_nothing() {
    // The path names a file that does not exist. Evaluation still answers,
    // because evaluation does not touch the filesystem.
    let map = MapLocal::new("/definitely/not/here.json").with_content_type("application/json");
    let rules = RuleSet::new([rule(
        "fixture",
        RuleMatcher::any().with_path("/users"),
        RuleAction::MapLocal(map.clone()),
        0,
    )]);

    match rules.evaluate(&request("GET", "api.example.com", "/users")) {
        RuleDecision::MapLocal { map: decided, .. } => assert_eq!(decided, map),
        other => panic!("expected a map local, got {other:?}"),
    }
}

#[test]
fn a_map_remote_carries_the_destination_it_was_given() {
    let rules = RuleSet::new([rule(
        "staging",
        RuleMatcher::any().with_host("api.example.com"),
        RuleAction::map_remote("https://staging.example.com:8443").expect("a valid destination"),
        0,
    )]);

    match rules.evaluate(&request("GET", "api.example.com", "/users")) {
        RuleDecision::MapRemote { map, .. } => {
            assert_eq!(map.target.host, "staging.example.com");
            assert_eq!(map.target.port, Some(8443));
            assert_eq!(map.target.path, None);
        }
        other => panic!("expected a map remote, got {other:?}"),
    }
}

#[test]
fn evaluating_the_same_request_twice_gives_the_same_answer() {
    let rules = RuleSet::new([blocking(users_matcher(), 0)]);
    let request = request("GET", "api.example.com", "/users");

    assert_eq!(rules.evaluate(&request), rules.evaluate(&request));
}

#[test]
fn a_decision_names_a_rule_that_is_in_the_set() {
    let blocked = blocking(users_matcher(), 0);
    let rules = RuleSet::new([blocked.clone()]);

    let decided = rules
        .evaluate(&request("GET", "api.example.com", "/users"))
        .rule()
        .expect("a rule decided");

    assert!(rules.rules().iter().any(|rule| rule.id == decided));
}

#[test]
fn a_rule_id_read_back_from_its_text_is_the_same_id() {
    let id = RuleId::next();

    assert_eq!(id.as_text().parse::<RuleId>().expect("parses"), id);
}

#[test]
fn a_map_remote_target_keeps_the_request_path_when_it_names_none() {
    let target = RemoteTarget::parse("https://staging.example.com").expect("a valid destination");

    assert_eq!(target.rewrite("/foo", Some("a=1")), "/foo?a=1");
    assert_eq!(target.rewrite("/foo", None), "/foo");
}
