//! Listing, paging, counting, and filtering stored traffic.

mod support;

use support::{
    at, failed_exchange, http_exchange, https_exchange, started_at, with_method, TestStore,
};

use proxy_storage::{
    ExchangeId, ExchangeSummary, StatusFilter, Storage, TrafficErrorKind, TrafficExchange,
    TrafficFilter, TrafficQuery, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT,
};

/// Stores every exchange, in the order given.
fn store_all(storage: &Storage, exchanges: &[TrafficExchange]) {
    for exchange in exchanges {
        storage.insert(exchange).expect("stores");
    }
}

/// The ids of a page, in the order the page holds them.
fn ids(page: &[ExchangeSummary]) -> Vec<ExchangeId> {
    page.iter().map(|summary| summary.id).collect()
}

/// Every exchange the filter matches, newest first, however many there are.
fn all_matching(storage: &Storage, filter: TrafficFilter) -> Vec<ExchangeSummary> {
    storage
        .list(
            &TrafficQuery::newest()
                .with_filter(filter)
                .with_limit(MAX_PAGE_LIMIT),
        )
        .expect("lists")
        .exchanges
}

/// Three exchanges a minute apart, oldest first in the returned order.
fn spread() -> Vec<TrafficExchange> {
    vec![
        started_at(http_exchange("one.example.com", "/a", 200), at(1_000)),
        started_at(http_exchange("two.example.com", "/b", 404), at(1_060)),
        started_at(http_exchange("three.example.com", "/c", 500), at(1_120)),
    ]
}

#[test]
fn an_exchange_is_read_back_by_its_id() {
    let store = TestStore::open();
    let exchanges = spread();
    store_all(&store.storage, &exchanges);

    let stored = store
        .storage
        .get(exchanges[1].id)
        .expect("reads")
        .expect("the exchange is stored");

    assert_eq!(stored, exchanges[1]);
}

#[test]
fn listing_returns_every_stored_exchange() {
    let store = TestStore::open();
    let exchanges = spread();
    store_all(&store.storage, &exchanges);

    let page = store.storage.list(&TrafficQuery::newest()).expect("lists");

    assert_eq!(page.len(), 3);
    assert_eq!(
        ids(&page.exchanges).len(),
        ids(&page.exchanges)
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        "an exchange was listed twice"
    );
}

#[test]
fn the_newest_exchange_is_listed_first() {
    let store = TestStore::open();
    let exchanges = spread();
    store_all(&store.storage, &exchanges);

    let page = store.storage.list(&TrafficQuery::newest()).expect("lists");

    assert_eq!(
        ids(&page.exchanges),
        vec![exchanges[2].id, exchanges[1].id, exchanges[0].id]
    );
}

#[test]
fn the_newest_exchange_is_listed_first_whatever_order_it_was_stored_in() {
    let store = TestStore::open();
    let mut exchanges = spread();
    // Stored newest first, so a store that listed rows in insertion order
    // would look right by accident.
    exchanges.reverse();
    store_all(&store.storage, &exchanges);

    let page = store.storage.list(&TrafficQuery::newest()).expect("lists");

    assert_eq!(
        ids(&page.exchanges),
        vec![exchanges[0].id, exchanges[1].id, exchanges[2].id]
    );
}

#[test]
fn exchanges_that_started_in_the_same_millisecond_are_still_in_a_settled_order() {
    let store = TestStore::open();
    let moment = at(2_000);
    let exchanges: Vec<TrafficExchange> = (0..16)
        .map(|index| {
            started_at(
                http_exchange("example.com", &format!("/{index}"), 200),
                moment,
            )
        })
        .collect();
    store_all(&store.storage, &exchanges);

    let first = store.storage.list(&TrafficQuery::newest()).expect("lists");
    let again = store.storage.list(&TrafficQuery::newest()).expect("lists");

    assert_eq!(ids(&first.exchanges), ids(&again.exchanges));
    // Ids are ordered by when they were minted, so the tie-break is the
    // order the exchanges happened in — newest first, like everything else.
    let mut expected: Vec<ExchangeId> = exchanges.iter().map(|exchange| exchange.id).collect();
    expected.reverse();
    assert_eq!(ids(&first.exchanges), expected);
}

#[test]
fn a_page_holds_what_was_asked_for_and_says_where_to_carry_on() {
    let store = TestStore::open();
    let exchanges = spread();
    store_all(&store.storage, &exchanges);

    let first = store
        .storage
        .list(&TrafficQuery::newest().with_limit(2))
        .expect("lists");

    assert_eq!(first.len(), 2);
    assert_eq!(
        ids(&first.exchanges),
        vec![exchanges[2].id, exchanges[1].id]
    );
    let cursor = first.next.expect("there is more to come");
    assert_eq!(cursor.id(), exchanges[1].id);

    let second = store
        .storage
        .list(&TrafficQuery::newest().with_limit(2).after(cursor))
        .expect("lists");

    assert_eq!(ids(&second.exchanges), vec![exchanges[0].id]);
    assert_eq!(second.next, None, "a page that was not filled is the last");
}

#[test]
fn paging_through_everything_sees_every_exchange_exactly_once() {
    let store = TestStore::open();
    let exchanges: Vec<TrafficExchange> = (0..25)
        .map(|index| {
            started_at(
                http_exchange("example.com", &format!("/{index}"), 200),
                at(1_000 + index),
            )
        })
        .collect();
    store_all(&store.storage, &exchanges);

    let mut seen: Vec<ExchangeId> = Vec::new();
    let mut query = TrafficQuery::newest().with_limit(4);
    loop {
        let page = store.storage.list(&query).expect("lists");
        seen.extend(ids(&page.exchanges));

        match page.next {
            Some(cursor) => query = query.after(cursor),
            None => break,
        }
    }

    let mut expected: Vec<ExchangeId> = exchanges.iter().map(|exchange| exchange.id).collect();
    expected.reverse();
    assert_eq!(seen, expected);
}

#[test]
fn paging_is_not_disturbed_by_traffic_arriving_while_it_happens() {
    let store = TestStore::open();
    let exchanges: Vec<TrafficExchange> = (0..6)
        .map(|index| {
            started_at(
                http_exchange("example.com", &format!("/{index}"), 200),
                at(1_000 + index),
            )
        })
        .collect();
    store_all(&store.storage, &exchanges);

    let first = store
        .storage
        .list(&TrafficQuery::newest().with_limit(3))
        .expect("lists");
    let cursor = first.next.expect("there is more to come");

    // Newer traffic, stored between the two pages. With an offset it would
    // push the second page along and repeat a row; with a cursor the second
    // page is the same rows it always was, and the new traffic is at the top
    // of the first page.
    store
        .storage
        .insert(&started_at(
            http_exchange("late.example.com", "/late", 200),
            at(9_000),
        ))
        .expect("stores");

    let second = store
        .storage
        .list(&TrafficQuery::newest().with_limit(3).after(cursor))
        .expect("lists");

    assert_eq!(
        ids(&second.exchanges),
        vec![exchanges[2].id, exchanges[1].id, exchanges[0].id]
    );
    let refreshed = store
        .storage
        .list(&TrafficQuery::newest().with_limit(1))
        .expect("lists");
    assert_eq!(refreshed.exchanges[0].url, "http://late.example.com/late");
}

#[test]
fn a_page_never_holds_more_than_a_page_can() {
    let store = TestStore::open();
    store_all(&store.storage, &spread());

    let asked_for_everything = store
        .storage
        .list(&TrafficQuery::newest().with_limit(usize::MAX))
        .expect("lists");

    assert_eq!(asked_for_everything.len(), 3);
    assert_eq!(TrafficQuery::newest().limit, DEFAULT_PAGE_LIMIT);
}

#[test]
fn listing_an_empty_database_is_an_empty_page() {
    let store = TestStore::open();

    let page = store.storage.list(&TrafficQuery::newest()).expect("lists");

    assert!(page.is_empty());
    assert_eq!(page.next, None);
}

#[test]
fn counting_says_how_many_are_stored() {
    let store = TestStore::open();

    assert_eq!(store.storage.count().expect("counts"), 0);

    store_all(&store.storage, &spread());

    assert_eq!(store.storage.count().expect("counts"), 3);
}

#[test]
fn counting_can_be_narrowed_the_same_way_listing_is() {
    let store = TestStore::open();
    store_all(&store.storage, &spread());

    assert_eq!(
        store
            .storage
            .count_matching(&TrafficFilter::all().with_host("two.example.com"))
            .expect("counts"),
        1
    );
    assert_eq!(
        store
            .storage
            .count_matching(&TrafficFilter::all().with_host("nowhere.example.com"))
            .expect("counts"),
        0
    );
}

#[test]
fn a_host_filter_returns_only_that_hosts_traffic() {
    let store = TestStore::open();
    let exchanges = vec![
        http_exchange("api.example.com", "/one", 200),
        http_exchange("api.example.com", "/two", 200),
        http_exchange("cdn.example.com", "/three", 200),
    ];
    store_all(&store.storage, &exchanges);

    let matched = all_matching(
        &store.storage,
        TrafficFilter::all().with_host("api.example.com"),
    );

    assert_eq!(matched.len(), 2);
    assert!(matched.iter().all(|summary| summary
        .destination
        .as_ref()
        .expect("a destination")
        .host
        == "api.example.com"));
}

#[test]
fn a_host_filter_does_not_care_how_the_host_was_spelled() {
    let store = TestStore::open();
    store
        .storage
        .insert(&http_exchange("API.Example.COM", "/x", 200))
        .expect("stores");

    let matched = all_matching(
        &store.storage,
        TrafficFilter::all().with_host("api.example.com"),
    );

    assert_eq!(matched.len(), 1);
    // Stored as it was captured, matched without regard to case.
    assert_eq!(
        matched[0].destination.as_ref().expect("a destination").host,
        "API.Example.COM"
    );
}

#[test]
fn a_method_filter_returns_only_that_methods_requests() {
    let store = TestStore::open();
    store_all(
        &store.storage,
        &[
            with_method(http_exchange("example.com", "/one", 200), "GET"),
            with_method(http_exchange("example.com", "/two", 200), "POST"),
            with_method(http_exchange("example.com", "/three", 200), "GET"),
        ],
    );

    let matched = all_matching(&store.storage, TrafficFilter::all().with_method("GET"));

    assert_eq!(matched.len(), 2);
    assert!(matched.iter().all(|summary| summary.method == "GET"));
}

#[test]
fn a_status_filter_returns_only_the_responses_it_names() {
    let store = TestStore::open();
    store_all(
        &store.storage,
        &[
            http_exchange("example.com", "/ok", 200),
            http_exchange("example.com", "/missing", 404),
            http_exchange("example.com", "/gone", 410),
            http_exchange("example.com", "/broken", 500),
        ],
    );

    let not_found = all_matching(
        &store.storage,
        TrafficFilter::all().with_status(StatusFilter::Exact(404)),
    );
    let client_errors = all_matching(
        &store.storage,
        TrafficFilter::all().with_status(StatusFilter::Class(4)),
    );

    assert_eq!(
        not_found
            .iter()
            .map(|summary| summary.status)
            .collect::<Vec<_>>(),
        vec![Some(404)]
    );
    assert_eq!(client_errors.len(), 2);
    assert!(client_errors
        .iter()
        .all(|summary| matches!(summary.status, Some(status) if (400..500).contains(&status))));
}

#[test]
fn a_status_filter_leaves_out_exchanges_that_never_got_a_response() {
    let store = TestStore::open();
    store_all(
        &store.storage,
        &[
            http_exchange("example.com", "/missing", 404),
            failed_exchange("example.com", TrafficErrorKind::UpstreamConnect),
        ],
    );

    let client_errors = all_matching(
        &store.storage,
        TrafficFilter::all().with_status(StatusFilter::Class(4)),
    );

    assert_eq!(client_errors.len(), 1);
    assert_eq!(client_errors[0].status, Some(404));
}

#[test]
fn a_time_range_returns_only_the_traffic_inside_it() {
    let store = TestStore::open();
    let exchanges = spread();
    store_all(&store.storage, &exchanges);

    let matched = all_matching(
        &store.storage,
        TrafficFilter::all().with_time_range(Some(at(1_060)), Some(at(1_120))),
    );

    // The start is included and the end is not.
    assert_eq!(ids(&matched), vec![exchanges[1].id]);
}

#[test]
fn a_time_range_can_be_open_at_either_end() {
    let store = TestStore::open();
    let exchanges = spread();
    store_all(&store.storage, &exchanges);

    let since = all_matching(
        &store.storage,
        TrafficFilter::all().with_time_range(Some(at(1_060)), None),
    );
    let until = all_matching(
        &store.storage,
        TrafficFilter::all().with_time_range(None, Some(at(1_060))),
    );

    assert_eq!(ids(&since), vec![exchanges[2].id, exchanges[1].id]);
    assert_eq!(ids(&until), vec![exchanges[0].id]);
}

#[test]
fn filters_narrow_each_other() {
    let store = TestStore::open();
    store_all(
        &store.storage,
        &[
            // The one that matches everything asked for below.
            started_at(
                with_method(http_exchange("api.example.com", "/wanted", 404), "GET"),
                at(1_060),
            ),
            // Right host and method, wrong status.
            started_at(
                with_method(http_exchange("api.example.com", "/ok", 200), "GET"),
                at(1_060),
            ),
            // Right host and status, wrong method.
            started_at(
                with_method(http_exchange("api.example.com", "/posted", 404), "POST"),
                at(1_060),
            ),
            // Right method and status, wrong host.
            started_at(
                with_method(http_exchange("cdn.example.com", "/missing", 404), "GET"),
                at(1_060),
            ),
            // Right in every way but the time.
            started_at(
                with_method(http_exchange("api.example.com", "/early", 404), "GET"),
                at(500),
            ),
        ],
    );

    let matched = all_matching(
        &store.storage,
        TrafficFilter::all()
            .with_host("api.example.com")
            .with_method("GET")
            .with_status(StatusFilter::Class(4))
            .with_time_range(Some(at(1_000)), Some(at(2_000))),
    );

    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].url, "http://api.example.com/wanted");
}

#[test]
fn a_filter_that_matches_nothing_returns_nothing() {
    let store = TestStore::open();
    store_all(&store.storage, &spread());

    let matched = all_matching(
        &store.storage,
        TrafficFilter::all().with_host("nowhere.example.com"),
    );

    assert!(matched.is_empty());
}

#[test]
fn paging_a_filtered_list_stays_filtered() {
    let store = TestStore::open();
    let wanted: Vec<TrafficExchange> = (0..5)
        .map(|index| {
            started_at(
                http_exchange("api.example.com", &format!("/{index}"), 200),
                at(1_000 + index),
            )
        })
        .collect();
    store_all(&store.storage, &wanted);
    store_all(
        &store.storage,
        &(0..5)
            .map(|index| {
                started_at(
                    http_exchange("cdn.example.com", &format!("/{index}"), 200),
                    at(1_000 + index),
                )
            })
            .collect::<Vec<_>>(),
    );

    let filter = TrafficFilter::all().with_host("api.example.com");
    let first = store
        .storage
        .list(
            &TrafficQuery::newest()
                .with_filter(filter.clone())
                .with_limit(2),
        )
        .expect("lists");
    let second = store
        .storage
        .list(
            &TrafficQuery::newest()
                .with_filter(filter)
                .with_limit(2)
                .after(first.next.expect("there is more")),
        )
        .expect("lists");

    for summary in first.exchanges.iter().chain(second.exchanges.iter()) {
        assert_eq!(
            summary.destination.as_ref().expect("a destination").host,
            "api.example.com"
        );
    }
    assert_eq!(ids(&second.exchanges), vec![wanted[2].id, wanted[1].id]);
}

#[test]
fn a_listed_exchange_says_what_a_traffic_list_shows() {
    let store = TestStore::open();
    let exchange = https_exchange("api.example.com", "/users", 201);
    store.storage.insert(&exchange).expect("stores");

    let page = store.storage.list(&TrafficQuery::newest()).expect("lists");
    let summary = &page.exchanges[0];

    assert_eq!(summary.id, exchange.id);
    assert_eq!(summary.method, "GET");
    assert_eq!(summary.url, "https://api.example.com/users");
    assert_eq!(summary.status, Some(201));
    assert_eq!(summary.reason.as_deref(), None);
    assert_eq!(summary.scheme, proxy_storage::Scheme::Https);
    assert_eq!(summary.client, support::client());
    assert_eq!(summary.started_at, exchange.timing.started_at);
    assert_eq!(summary.duration_micros, Some(3_500));
    assert_eq!(summary.duration_ms(), Some(3.5));
    assert!(summary.completed);
    assert!(summary.succeeded());
    assert_eq!(summary.error, None);
    // The response head arrived two milliseconds after the request did.
    assert_eq!(
        summary.response_at.expect("a response time").as_millis(),
        exchange.timing.started_at.as_millis() + 2
    );
}

#[test]
fn a_summary_built_from_an_exchange_is_the_one_the_list_returns() {
    let store = TestStore::open();
    // Every shape a summary has to survive: a finished exchange, one that
    // failed, and one that never got an answer at all.
    let exchanges = vec![
        https_exchange("api.example.com", "/users", 201),
        failed_exchange("example.com", TrafficErrorKind::UpstreamConnect),
        support::unanswered_exchange("example.com"),
    ];
    store_all(&store.storage, &exchanges);

    let listed = all_matching(&store.storage, TrafficFilter::all());

    for exchange in &exchanges {
        let stored = listed
            .iter()
            .find(|summary| summary.id == exchange.id)
            .expect("every exchange is listed");

        assert_eq!(*stored, ExchangeSummary::of(exchange));
    }
}

#[test]
fn a_listed_exchange_that_failed_says_why() {
    let store = TestStore::open();
    let exchange = failed_exchange("example.com", TrafficErrorKind::UpstreamConnect);
    store.storage.insert(&exchange).expect("stores");

    let page = store.storage.list(&TrafficQuery::newest()).expect("lists");
    let summary = &page.exchanges[0];

    assert_eq!(summary.status, None);
    assert!(!summary.succeeded());
    assert_eq!(
        summary
            .error
            .as_ref()
            .map(proxy_storage::TrafficError::kind),
        Some(TrafficErrorKind::UpstreamConnect)
    );
}

#[test]
fn a_listed_exchange_that_never_finished_says_so() {
    let store = TestStore::open();
    let exchange = support::unanswered_exchange("example.com");
    store.storage.insert(&exchange).expect("stores");

    let page = store.storage.list(&TrafficQuery::newest()).expect("lists");
    let summary = &page.exchanges[0];

    assert!(!summary.completed);
    assert_eq!(summary.duration_micros, None);
    assert_eq!(summary.duration_ms(), None);
    assert_eq!(summary.response_at, None);
}

// Searching and reading the order from the other end. Both narrow the same
// query the filters above do, and neither changes what a page is.

/// Every exchange the filter matches, oldest first.
fn all_matching_oldest(storage: &Storage, filter: TrafficFilter) -> Vec<ExchangeSummary> {
    storage
        .list(
            &TrafficQuery::oldest()
                .with_filter(filter)
                .with_limit(MAX_PAGE_LIMIT),
        )
        .expect("lists")
        .exchanges
}

#[test]
fn a_search_matches_part_of_a_host() {
    let store = TestStore::open();
    store_all(&store.storage, &spread());

    let found = all_matching(
        &store.storage,
        TrafficFilter::all().with_text("two.example"),
    );

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].url, "http://two.example.com/b");
}

#[test]
fn a_search_matches_part_of_a_url() {
    let store = TestStore::open();
    store_all(
        &store.storage,
        &[
            http_exchange("api.example.com", "/v1/users/login", 200),
            http_exchange("api.example.com", "/v1/orders", 200),
        ],
    );

    let found = all_matching(&store.storage, TrafficFilter::all().with_text("login"));

    assert_eq!(found.len(), 1);
    assert!(
        found[0].url.ends_with("/v1/users/login"),
        "{}",
        found[0].url
    );
}

#[test]
fn a_search_matches_a_method() {
    let store = TestStore::open();
    store_all(
        &store.storage,
        &[
            with_method(http_exchange("one.example.com", "/a", 200), "DELETE"),
            http_exchange("two.example.com", "/b", 200),
        ],
    );

    let found = all_matching(&store.storage, TrafficFilter::all().with_text("delete"));

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].method, "DELETE");
}

#[test]
fn a_search_matches_a_status_code_written_out() {
    // Someone typing `404` into a search box means the responses that were
    // one, and the status is a number in the row rather than text.
    let store = TestStore::open();
    store_all(&store.storage, &spread());

    let found = all_matching(&store.storage, TrafficFilter::all().with_text("404"));

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].status, Some(404));
}

#[test]
fn a_search_does_not_care_about_case() {
    let store = TestStore::open();
    store_all(
        &store.storage,
        &[http_exchange("API.Example.com", "/V1/Users", 200)],
    );

    for search in ["api.example", "API.EXAMPLE", "/v1/users", "/V1/USERS"] {
        assert_eq!(
            all_matching(&store.storage, TrafficFilter::all().with_text(search)).len(),
            1,
            "{search}"
        );
    }
}

#[test]
fn a_search_that_matches_nothing_is_an_empty_page() {
    let store = TestStore::open();
    store_all(&store.storage, &spread());

    assert!(all_matching(&store.storage, TrafficFilter::all().with_text("nowhere")).is_empty());
}

#[test]
fn a_searchs_wildcards_are_the_characters_they_are() {
    // `%` and `_` mean something to `LIKE`, and nothing to a person typing
    // them into a search box.
    let store = TestStore::open();
    store_all(
        &store.storage,
        &[
            http_exchange("one.example.com", "/50-off", 200),
            http_exchange("two.example.com", "/a_b", 200),
            http_exchange("three.example.com", "/axb", 200),
        ],
    );

    // As a wildcard, `5%f` would match `/50-off`. As text, it matches
    // nothing, because nothing contains those three characters in a row.
    let percent = all_matching(&store.storage, TrafficFilter::all().with_text("5%f"));
    assert!(percent.is_empty(), "{percent:?}");

    // As a wildcard, `_` would match the `x` in `/axb` as well.
    let underscore = all_matching(&store.storage, TrafficFilter::all().with_text("a_b"));
    assert_eq!(underscore.len(), 1);
    assert!(underscore[0].url.ends_with("/a_b"), "{}", underscore[0].url);

    // The escape character itself: nothing contains one, and asking is not
    // an error.
    let backslash = all_matching(&store.storage, TrafficFilter::all().with_text("\\"));
    assert!(backslash.is_empty(), "{backslash:?}");
}

#[test]
fn a_search_narrows_alongside_the_other_filters_rather_than_replacing_them() {
    let store = TestStore::open();
    store_all(
        &store.storage,
        &[
            with_method(http_exchange("api.example.com", "/users", 200), "POST"),
            http_exchange("api.example.com", "/users", 200),
            with_method(http_exchange("other.example.com", "/users", 200), "POST"),
        ],
    );

    let found = all_matching(
        &store.storage,
        TrafficFilter::all()
            .with_text("users")
            .with_method("POST")
            .with_host("api.example.com")
            .with_status(StatusFilter::Class(2)),
    );

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].method, "POST");
    assert_eq!(
        found[0].destination.as_ref().map(|to| to.host.as_str()),
        Some("api.example.com")
    );
}

#[test]
fn a_search_finds_an_exchange_that_never_got_a_response() {
    let store = TestStore::open();
    store_all(
        &store.storage,
        &[failed_exchange(
            "gone.example.com",
            TrafficErrorKind::UpstreamConnect,
        )],
    );

    assert_eq!(
        all_matching(
            &store.storage,
            TrafficFilter::all().with_text("gone.example")
        )
        .len(),
        1
    );
}

#[test]
fn the_oldest_exchange_is_listed_first_when_the_order_is_read_that_way() {
    let store = TestStore::open();
    let exchanges = spread();
    store_all(&store.storage, &exchanges);

    let oldest = all_matching_oldest(&store.storage, TrafficFilter::all());

    assert_eq!(
        ids(&oldest),
        vec![exchanges[0].id, exchanges[1].id, exchanges[2].id]
    );
}

#[test]
fn reading_the_order_from_either_end_gives_the_same_exchanges_reversed() {
    let store = TestStore::open();
    store_all(&store.storage, &spread());

    let mut oldest = ids(&all_matching_oldest(&store.storage, TrafficFilter::all()));
    oldest.reverse();

    assert_eq!(
        ids(&all_matching(&store.storage, TrafficFilter::all())),
        oldest
    );
}

#[test]
fn exchanges_from_one_millisecond_stay_settled_read_from_either_end() {
    let store = TestStore::open();
    let together: Vec<TrafficExchange> = (0..8)
        .map(|_| started_at(http_exchange("one.example.com", "/a", 200), at(1_000)))
        .collect();
    store_all(&store.storage, &together);

    let newest = ids(&all_matching(&store.storage, TrafficFilter::all()));
    let mut oldest = ids(&all_matching_oldest(&store.storage, TrafficFilter::all()));
    oldest.reverse();

    // Ids break the tie, and they break it the same way in both directions.
    assert_eq!(newest, oldest);
}

#[test]
fn paging_oldest_first_sees_every_exchange_exactly_once() {
    let store = TestStore::open();
    let exchanges: Vec<TrafficExchange> = (0..25)
        .map(|index| {
            started_at(
                http_exchange("one.example.com", "/a", 200),
                at(1_000 + index),
            )
        })
        .collect();
    store_all(&store.storage, &exchanges);

    let mut seen: Vec<ExchangeId> = Vec::new();
    let mut query = TrafficQuery::oldest().with_limit(7);

    loop {
        let page = store.storage.list(&query).expect("lists");
        if page.is_empty() {
            break;
        }

        seen.extend(ids(&page.exchanges));

        match page.next {
            Some(cursor) => query = query.after(cursor),
            None => break,
        }
    }

    let expected: Vec<ExchangeId> = exchanges.iter().map(|exchange| exchange.id).collect();
    assert_eq!(seen, expected);
}

#[test]
fn paging_oldest_first_carries_the_filters_with_it() {
    let store = TestStore::open();
    let mut exchanges: Vec<TrafficExchange> = Vec::new();
    for index in 0..10 {
        exchanges.push(started_at(
            http_exchange("keep.example.com", "/a", 200),
            at(1_000 + index * 2),
        ));
        exchanges.push(started_at(
            http_exchange("drop.example.com", "/a", 200),
            at(1_001 + index * 2),
        ));
    }
    store_all(&store.storage, &exchanges);

    let mut seen = 0;
    let mut query = TrafficQuery::oldest()
        .with_limit(3)
        .with_filter(TrafficFilter::all().with_host("keep.example.com"));

    loop {
        let page = store.storage.list(&query).expect("lists");
        seen += page.len();

        match page.next {
            Some(cursor) => query = query.after(cursor),
            None => break,
        }
    }

    assert_eq!(seen, 10);
}

#[test]
fn changing_the_order_starts_the_paging_again() {
    // A cursor names a place in the order it was produced in, so carrying one
    // across a change of direction would ask for the rows on the other side
    // of it — a different page than the caller meant.
    let store = TestStore::open();
    store_all(&store.storage, &spread());

    let first = store
        .storage
        .list(&TrafficQuery::newest().with_limit(1))
        .expect("lists");
    let cursor = first.next.expect("a full page has a cursor");

    let carried = TrafficQuery::newest()
        .with_limit(1)
        .after(cursor)
        .with_order(proxy_storage::TrafficOrder::Oldest);

    assert!(carried.after.is_none());
}

#[test]
fn counting_matches_what_a_search_lists() {
    let store = TestStore::open();
    store_all(&store.storage, &spread());

    let filter = TrafficFilter::all().with_text("example.com");

    assert_eq!(
        store.storage.count_matching(&filter).expect("counts"),
        all_matching(&store.storage, filter.clone()).len() as u64
    );
}
