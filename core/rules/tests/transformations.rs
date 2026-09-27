//! Transformations and the decisions around them.
//!
//! What is pinned here is the ordering, because it is the part a person
//! reasons about: a transformation above the deciding rule is applied, one
//! below it is not, a decision above one exempts what it matches, and where
//! two change the same thing the one higher in the list wins. And the JSON
//! the three new actions are stored and sent as, because a rule whose
//! encoding changed silently is a rule an older database cannot read.

use rules::{
    BodyEdit, HeaderEdit, ModifyRequest, ModifyResponse, NewRule, PathEdit, QueryEdit, Redirect,
    RemoteTarget, RequestContext, Rule, RuleAction, RuleActionError, RuleDecision, RuleMatcher,
    RuleSet, DEFAULT_REDIRECT_STATUS,
};

fn rule(name: &str, matcher: RuleMatcher, action: RuleAction, priority: i32) -> Rule {
    Rule::create(NewRule::new(name, matcher, action).with_priority(priority)).expect("a valid rule")
}

fn request<'a>() -> RequestContext<'a> {
    RequestContext::new("api.example.com", "/users", "GET")
}

/// A Modify Request rule that sets one header.
fn setting(name: &str, value: &str) -> RuleAction {
    RuleAction::ModifyRequest(ModifyRequest {
        headers: vec![HeaderEdit::Set {
            name: name.into(),
            value: value.into(),
        }],
        ..ModifyRequest::default()
    })
}

/// A Modify Response rule that sets the status.
fn answering(status: u16) -> RuleAction {
    RuleAction::ModifyResponse(ModifyResponse {
        status: Some(status),
        ..ModifyResponse::default()
    })
}

fn value_of(change: &ModifyRequest) -> &str {
    match &change.headers[0] {
        HeaderEdit::Set { value, .. } => value,
        other => panic!("not a set: {other:?}"),
    }
}

// MARK: - Ordering

#[test]
fn a_transformation_above_the_decision_is_applied_and_the_decision_still_decides() {
    let modify = rule("auth", RuleMatcher::any(), setting("Authorization", "a"), 0);
    let map = rule(
        "staging",
        RuleMatcher::any(),
        RuleAction::map_remote("https://staging.example.com").expect("a destination"),
        10,
    );
    let rules = RuleSet::new([modify.clone(), map.clone()]);

    let outcome = rules.outcome(&request());

    assert_eq!(outcome.decision.rule(), Some(map.id));
    assert!(matches!(outcome.decision, RuleDecision::MapRemote { .. }));
    assert_eq!(outcome.request.len(), 1);
    assert_eq!(outcome.request[0].rule, modify.id);
}

#[test]
fn a_transformation_below_the_decision_is_never_consulted() {
    let block = rule("block", RuleMatcher::any(), RuleAction::block(), 0);
    let modify = rule(
        "auth",
        RuleMatcher::any(),
        setting("Authorization", "a"),
        10,
    );
    let rules = RuleSet::new([modify, block.clone()]);

    let outcome = rules.outcome(&request());

    assert_eq!(outcome.decision.rule(), Some(block.id));
    assert!(outcome.request.is_empty());
}

#[test]
fn an_allow_above_a_transformation_exempts_what_it_matches() {
    let allow = rule(
        "health",
        RuleMatcher::any().with_path("/users"),
        RuleAction::Allow,
        0,
    );
    let modify = rule(
        "auth",
        RuleMatcher::any(),
        setting("Authorization", "a"),
        10,
    );
    let rules = RuleSet::new([allow, modify]);

    assert!(rules.outcome(&request()).request.is_empty());
    assert_eq!(
        rules
            .outcome(&RequestContext::new("api.example.com", "/other", "GET"))
            .request
            .len(),
        1
    );
}

#[test]
fn transformations_with_no_decision_leave_the_request_allowed() {
    let rules = RuleSet::new([
        rule("req", RuleMatcher::any(), setting("X-A", "1"), 0),
        rule("res", RuleMatcher::any(), answering(404), 1),
    ]);

    let outcome = rules.outcome(&request());

    assert_eq!(outcome.decision, RuleDecision::allowed());
    assert_eq!(outcome.request.len(), 1);
    assert_eq!(outcome.response.len(), 1);
}

#[test]
fn evaluate_passes_transformations_over_and_returns_the_decision() {
    let modify = rule("auth", RuleMatcher::any(), setting("Authorization", "a"), 0);
    let block = rule("block", RuleMatcher::any(), RuleAction::block(), 10);
    let rules = RuleSet::new([modify, block.clone()]);

    assert_eq!(rules.evaluate(&request()).rule(), Some(block.id));
}

#[test]
fn where_two_change_the_same_thing_the_one_higher_in_the_list_wins() {
    let high = rule("high", RuleMatcher::any(), setting("X-Env", "high"), 0);
    let low = rule("low", RuleMatcher::any(), setting("X-Env", "low"), 10);
    let rules = RuleSet::new([low, high.clone()]);

    let outcome = rules.outcome(&request());

    // Applied in order, so the last one applied is the one that stands.
    let last = outcome.request.last().expect("two transformations");
    assert_eq!(last.rule, high.id);
    assert_eq!(value_of(&last.change), "high");
}

#[test]
fn at_equal_priority_the_older_rule_is_higher_in_the_list() {
    let older = rule("older", RuleMatcher::any(), setting("X-Env", "older"), 5);
    let newer = rule("newer", RuleMatcher::any(), setting("X-Env", "newer"), 5);
    let rules = RuleSet::new([newer, older.clone()]);

    let outcome = rules.outcome(&request());

    assert_eq!(outcome.request.last().map(|t| t.rule), Some(older.id));
}

#[test]
fn a_disabled_transformation_is_not_collected() {
    let modify =
        Rule::create(NewRule::new("off", RuleMatcher::any(), setting("X-A", "1")).disabled())
            .expect("a valid rule");

    assert!(RuleSet::new([modify])
        .outcome(&request())
        .request
        .is_empty());
}

#[test]
fn a_transformation_is_matched_like_any_rule() {
    let rules = RuleSet::new([rule(
        "posts only",
        RuleMatcher::any().with_method("POST"),
        setting("X-A", "1"),
        0,
    )]);

    assert!(rules.outcome(&request()).request.is_empty());
    assert_eq!(
        rules
            .outcome(&RequestContext::new("api.example.com", "/users", "POST"))
            .request
            .len(),
        1
    );
}

#[test]
fn a_redirect_is_a_decision() {
    let redirect = rule(
        "moved",
        RuleMatcher::any(),
        RuleAction::redirect("https://new.example.com/users", 308).expect("a redirect"),
        0,
    );
    let below = rule("auth", RuleMatcher::any(), setting("X-A", "1"), 10);
    let rules = RuleSet::new([redirect.clone(), below]);

    let outcome = rules.outcome(&request());

    assert_eq!(outcome.decision.rule(), Some(redirect.id));
    assert!(!outcome.decision.is_carried());
    assert!(outcome.request.is_empty());
}

// MARK: - Redirect

#[test]
fn a_redirect_goes_exactly_where_it_says() {
    let RuleAction::Redirect(redirect) =
        RuleAction::redirect("https://new.example.com:8443/v2?x=1", 307).expect("a redirect")
    else {
        panic!("not a redirect");
    };

    assert_eq!(redirect.location(), "https://new.example.com:8443/v2?x=1");
    assert_eq!(redirect.status, 307);
}

#[test]
fn only_the_four_redirect_statuses_redirect() {
    for status in [300, 303, 304, 200, 404] {
        assert_eq!(
            RuleAction::redirect("https://example.com", status),
            Err(RuleActionError::RedirectStatus),
            "{status}"
        );
    }
    for status in [301, 302, 307, 308] {
        assert!(RuleAction::redirect("https://example.com", status).is_ok());
    }
}

#[test]
fn a_redirect_target_is_an_http_url() {
    for url in [
        "ftp://example.com",
        "example.com",
        "https://user@example.com",
        "",
    ] {
        assert_eq!(
            RuleAction::redirect(url, 302),
            Err(RuleActionError::RedirectTarget),
            "{url}"
        );
    }
}

// MARK: - On the wire

#[test]
fn a_modify_request_rule_is_written_as_its_edits() {
    let action = RuleAction::ModifyRequest(ModifyRequest {
        method: Some("POST".into()),
        path: Some(PathEdit::ReplacePrefix {
            from: "/v1/".into(),
            to: "/v2/".into(),
        }),
        query: vec![QueryEdit::Set {
            name: "env".into(),
            value: "staging".into(),
        }],
        headers: vec![HeaderEdit::Remove {
            name: "Cookie".into(),
        }],
        body: Some(BodyEdit::SetJson {
            path: "user.role".into(),
            value: "\"admin\"".into(),
        }),
    });

    let json = serde_json::to_value(&action).expect("serialises");

    assert_eq!(
        json,
        serde_json::json!({
            "type": "modify_request",
            "method": "POST",
            "path": {"op": "replace_prefix", "from": "/v1/", "to": "/v2/"},
            "query": [{"op": "set", "name": "env", "value": "staging"}],
            "headers": [{"op": "remove", "name": "Cookie"}],
            "body": {"op": "set_json", "path": "user.role", "value": "\"admin\""},
        })
    );
    assert_eq!(
        serde_json::from_value::<RuleAction>(json).expect("reads back"),
        action
    );
}

#[test]
fn a_modify_response_rule_leaves_out_what_it_does_not_change() {
    let json = serde_json::to_value(answering(404)).expect("serialises");

    assert_eq!(
        json,
        serde_json::json!({"type": "modify_response", "status": 404})
    );
}

#[test]
fn a_redirect_rule_is_written_as_its_target_and_status() {
    let redirect = RuleAction::redirect("https://new.example.com/", 301).expect("a redirect");

    assert_eq!(
        serde_json::to_value(&redirect).expect("serialises"),
        serde_json::json!({
            "type": "redirect",
            "target": {"scheme": "https", "host": "new.example.com"},
            "status": 301,
        })
    );
}

#[test]
fn a_redirect_that_names_no_status_is_temporary() {
    let read: RuleAction = serde_json::from_value(serde_json::json!({
        "type": "redirect",
        "target": {"scheme": "https", "host": "new.example.com"},
    }))
    .expect("reads");

    assert_eq!(
        read,
        RuleAction::Redirect(Redirect {
            target: RemoteTarget::parse("https://new.example.com").expect("a target"),
            status: DEFAULT_REDIRECT_STATUS,
        })
    );
}

#[test]
fn a_field_this_build_does_not_know_is_refused() {
    let read = serde_json::from_value::<RuleAction>(serde_json::json!({
        "type": "modify_request",
        "method": "POST",
        "script": "anything",
    }));

    assert!(read.is_err());
}

#[test]
fn a_transformation_that_changes_nothing_cannot_be_created() {
    let created = Rule::create(NewRule::new(
        "nothing",
        RuleMatcher::any(),
        RuleAction::ModifyRequest(ModifyRequest::default()),
    ));

    assert!(created.is_err());
}
