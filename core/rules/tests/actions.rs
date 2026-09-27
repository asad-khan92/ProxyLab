//! What an action accepts, what it refuses, and what it rewrites a request to.
//!
//! Map Remote's URL parsing is the security surface of this crate: a rule is
//! written once and then used for every matching request, so a destination
//! that is wrong is wrong for ever. It is checked when the rule is written.

use std::path::PathBuf;

use rules::{
    MapLocal, NewRule, RemoteTarget, Rule, RuleAction, RuleActionError, RuleError, RuleMatcher,
};

fn target(url: &str) -> RemoteTarget {
    RemoteTarget::parse(url).unwrap_or_else(|error| panic!("{url} should parse: {error}"))
}

fn refusal(url: &str) -> RuleActionError {
    RemoteTarget::parse(url).expect_err(&format!("{url} should be refused"))
}

// MARK: - Map Remote: what a destination is

#[test]
fn a_destination_names_a_scheme_a_host_and_maybe_a_port() {
    let plain = target("http://staging.example.com");
    assert_eq!(plain.host, "staging.example.com");
    assert_eq!(plain.port, None);
    assert_eq!(plain.effective_port(), 80);

    let secure = target("https://staging.example.com");
    assert_eq!(secure.effective_port(), 443);

    let ported = target("https://staging.example.com:8443");
    assert_eq!(ported.port, Some(8443));
    assert_eq!(ported.effective_port(), 8443);
}

#[test]
fn a_destination_may_be_an_address() {
    assert_eq!(target("http://127.0.0.1:8080").host, "127.0.0.1");
    // An IPv6 literal keeps its colons and loses its brackets, the way the
    // rest of the proxy stores a host.
    let six = target("http://[::1]:8080");
    assert_eq!(six.host, "::1");
    assert_eq!(six.port, Some(8080));
    assert_eq!(six.host_header(), "[::1]:8080");
}

#[test]
fn a_destination_may_name_a_path_and_a_query() {
    let with_path = target("https://staging.example.com/v2");
    assert_eq!(with_path.path.as_deref(), Some("/v2"));
    assert_eq!(with_path.query, None);

    let with_query = target("https://staging.example.com/v2?debug=1");
    assert_eq!(with_query.path.as_deref(), Some("/v2"));
    assert_eq!(with_query.query.as_deref(), Some("debug=1"));
}

#[test]
fn a_bare_slash_is_not_a_path() {
    // `https://host` and `https://host/` mean the same thing: keep the
    // request's path.
    assert_eq!(target("https://staging.example.com/").path, None);
    assert_eq!(target("https://staging.example.com").path, None);
}

#[test]
fn a_destination_that_is_not_one_is_refused() {
    assert_eq!(refusal(""), RuleActionError::RemoteUrlEmpty);
    assert_eq!(refusal("   "), RuleActionError::RemoteUrlEmpty);
    assert_eq!(
        refusal("staging.example.com"),
        RuleActionError::RemoteUrlNoScheme
    );
    assert_eq!(refusal("/just/a/path"), RuleActionError::RemoteUrlNoScheme);
    assert_eq!(refusal("https://"), RuleActionError::RemoteHostEmpty);
    assert_eq!(refusal("https:///path"), RuleActionError::RemoteHostEmpty);
}

#[test]
fn a_scheme_this_proxy_cannot_speak_is_refused() {
    // The point of refusing here rather than at request time: a rule that
    // cannot work should not be storable.
    for url in [
        "file:///etc/passwd",
        "ftp://example.com",
        "ws://example.com",
        "gopher://example.com",
        "HTTPX://example.com",
    ] {
        assert_eq!(
            RemoteTarget::parse(url),
            Err(RuleActionError::RemoteSchemeUnsupported),
            "{url}"
        );
    }
}

#[test]
fn a_scheme_is_read_however_it_was_spelled() {
    assert_eq!(target("HTTPS://staging.example.com").effective_port(), 443);
    assert_eq!(target("Http://staging.example.com").effective_port(), 80);
}

#[test]
fn a_destination_carrying_credentials_is_refused() {
    // Credentials in a rule would be credentials sent to a server on every
    // matching request, which is never what a mapping means.
    assert_eq!(
        refusal("https://user:secret@staging.example.com"),
        RuleActionError::RemoteUrlUserinfo
    );
}

#[test]
fn a_destination_carrying_a_fragment_is_refused() {
    assert_eq!(
        refusal("https://staging.example.com/v2#section"),
        RuleActionError::RemoteUrlFragment
    );
}

#[test]
fn a_destination_carrying_whitespace_or_control_characters_is_refused() {
    for url in [
        "https://staging.example.com/a b",
        "https://staging example.com",
        "https://staging.example.com/\nHost: evil",
        "https://staging.example.com/\rX",
    ] {
        assert_eq!(
            RemoteTarget::parse(url),
            Err(RuleActionError::RemoteUrlInvalid),
            "{url}"
        );
    }
}

#[test]
fn a_port_that_is_not_a_port_is_refused() {
    for url in [
        "https://staging.example.com:0",
        "https://staging.example.com:99999",
        "https://staging.example.com:http",
        "https://staging.example.com:-1",
    ] {
        assert!(
            matches!(
                RemoteTarget::parse(url),
                Err(RuleActionError::RemotePortInvalid)
            ),
            "{url} was not refused"
        );
    }
}

// MARK: - Map Remote: what a request becomes

#[test]
fn a_destination_with_no_path_keeps_the_whole_request_path() {
    // The example from the step: the one thing that must never happen is the
    // two URLs being pasted together.
    let target = target("https://staging.example.com");

    assert_eq!(target.rewrite("/foo", None), "/foo");
    assert_eq!(target.rewrite("/foo", Some("a=1")), "/foo?a=1");
    assert_eq!(target.rewrite("/", None), "/");
    assert_eq!(
        target.rewrite("/deep/path/here", Some("a=1&b=2")),
        "/deep/path/here?a=1&b=2"
    );
}

#[test]
fn a_destination_with_a_path_replaces_the_requests_path() {
    let target = target("https://staging.example.com/v2");

    assert_eq!(target.rewrite("/foo", None), "/v2");
    // The query is the request's own: a rule that redirects a path does not
    // silently drop `?debug=1`.
    assert_eq!(target.rewrite("/foo", Some("a=1")), "/v2?a=1");
}

#[test]
fn a_destination_with_a_query_replaces_the_requests_query() {
    let target = target("https://staging.example.com/v2?fixed=yes");

    assert_eq!(target.rewrite("/foo", Some("a=1")), "/v2?fixed=yes");
    assert_eq!(target.rewrite("/foo", None), "/v2?fixed=yes");
}

#[test]
fn the_host_header_names_the_port_only_when_it_is_not_the_schemes_own() {
    assert_eq!(
        target("https://staging.example.com").host_header(),
        "staging.example.com"
    );
    assert_eq!(
        target("https://staging.example.com:443").host_header(),
        "staging.example.com"
    );
    assert_eq!(
        target("https://staging.example.com:8443").host_header(),
        "staging.example.com:8443"
    );
    assert_eq!(
        target("http://staging.example.com:80").host_header(),
        "staging.example.com"
    );
}

#[test]
fn a_destination_reads_back_as_the_url_it_came_from() {
    for url in [
        "https://staging.example.com",
        "https://staging.example.com:8443",
        "https://staging.example.com/v2",
        "https://staging.example.com/v2?fixed=yes",
        "http://127.0.0.1:8080",
    ] {
        assert_eq!(target(url).to_string(), url, "{url}");
    }
}

// MARK: - Map Local

#[test]
fn a_map_local_names_an_absolute_file() {
    assert_eq!(MapLocal::new("/tmp/fixture.json").validate(), Ok(()));
}

#[test]
fn a_map_local_path_that_is_not_absolute_is_refused() {
    // There is no working directory a rule should silently depend on.
    assert_eq!(
        MapLocal::new("fixture.json").validate(),
        Err(RuleActionError::MapLocalPathNotAbsolute)
    );
    assert_eq!(
        MapLocal::new("../secrets").validate(),
        Err(RuleActionError::MapLocalPathNotAbsolute)
    );
    assert_eq!(
        MapLocal::new(PathBuf::new()).validate(),
        Err(RuleActionError::MapLocalPathEmpty)
    );
}

#[test]
fn a_map_local_content_type_must_be_a_header_value() {
    assert_eq!(
        MapLocal::new("/tmp/x")
            .with_content_type("application/json")
            .validate(),
        Ok(())
    );
    assert_eq!(
        MapLocal::new("/tmp/x").with_content_type("").validate(),
        Err(RuleActionError::MapLocalContentType)
    );
    assert_eq!(
        MapLocal::new("/tmp/x")
            .with_content_type("text/plain\r\nX-Evil: yes")
            .validate(),
        Err(RuleActionError::MapLocalContentType)
    );
}

#[test]
fn a_map_local_does_not_ask_whether_the_file_is_there() {
    // A rule is validated when it is written; the file is checked when the
    // rule runs. A rule that stops working because a file was deleted is a
    // runtime failure, not an invalid rule.
    assert_eq!(MapLocal::new("/definitely/not/here").validate(), Ok(()));
}

// MARK: - Block

#[test]
fn a_block_status_must_be_a_status() {
    assert_eq!(RuleAction::block_with(404).validate(), Ok(()));
    assert_eq!(RuleAction::block_with(451).validate(), Ok(()));
    assert_eq!(
        RuleAction::block_with(99).validate(),
        Err(RuleActionError::BlockStatus)
    );
    assert_eq!(
        RuleAction::block_with(600).validate(),
        Err(RuleActionError::BlockStatus)
    );
}

// MARK: - The rule around the action

#[test]
fn a_rule_must_have_a_name() {
    let unnamed = NewRule::new("", RuleMatcher::any(), RuleAction::Allow);

    assert_eq!(Rule::create(unnamed), Err(RuleError::NameEmpty));
    assert_eq!(
        Rule::create(NewRule::new("   ", RuleMatcher::any(), RuleAction::Allow)),
        Err(RuleError::NameEmpty)
    );
}

#[test]
fn a_rule_name_is_bounded_and_printable() {
    let long = "x".repeat(rules::MAX_RULE_NAME_LENGTH + 1);
    assert_eq!(
        Rule::create(NewRule::new(long, RuleMatcher::any(), RuleAction::Allow)),
        Err(RuleError::NameTooLong)
    );
    assert_eq!(
        Rule::create(NewRule::new(
            "two\nlines",
            RuleMatcher::any(),
            RuleAction::Allow
        )),
        Err(RuleError::NameInvalid)
    );
}

#[test]
fn a_matcher_that_could_never_match_is_refused() {
    let cases = [
        RuleMatcher::any().with_host(""),
        RuleMatcher::any().with_host("has space"),
        RuleMatcher::any().with_path("users"),
        RuleMatcher::any().with_path_prefix("no-slash"),
        RuleMatcher::any().with_method(""),
        RuleMatcher::any().with_method("GET POST"),
    ];

    for matcher in cases {
        assert_eq!(
            Rule::create(NewRule::new("rule", matcher.clone(), RuleAction::Allow)),
            Err(RuleError::Matcher),
            "{matcher:?}"
        );
    }
}

#[test]
fn a_rule_with_an_action_that_cannot_run_is_refused() {
    assert_eq!(
        Rule::create(NewRule::new(
            "rule",
            RuleMatcher::any(),
            RuleAction::block_with(0)
        )),
        Err(RuleError::Action(RuleActionError::BlockStatus))
    );
    assert_eq!(
        Rule::create(NewRule::new(
            "rule",
            RuleMatcher::any(),
            RuleAction::map_local("relative/path")
        )),
        Err(RuleError::Action(RuleActionError::MapLocalPathNotAbsolute))
    );
}

#[test]
fn a_rule_is_created_enabled_unless_it_says_otherwise() {
    let rule = Rule::create(NewRule::new("r", RuleMatcher::any(), RuleAction::Allow))
        .expect("a valid rule");
    assert!(rule.enabled);
    assert_eq!(rule.priority, 0);
    assert_eq!(rule.created_at_ms, rule.updated_at_ms);

    let off = Rule::create(NewRule::new("r", RuleMatcher::any(), RuleAction::Allow).disabled())
        .expect("a valid rule");
    assert!(!off.enabled);
}

#[test]
fn a_rules_summary_names_no_host_path_or_name() {
    let rule = Rule::create(NewRule::new(
        "secret project rule",
        RuleMatcher::any()
            .with_host("internal.example.com")
            .with_path("/secret"),
        RuleAction::block(),
    ))
    .expect("a valid rule");

    let summary = rule.summary();
    for private in ["secret project rule", "internal.example.com", "/secret"] {
        assert!(!summary.contains(private), "{summary} carries {private}");
    }
    assert!(summary.contains("block"), "{summary}");
}
