//! Deciding what happens to one request.
//!
//! ```text
//! rules, sorted once          request
//!   priority 5  /health  ──┐     │
//!   priority 10 *        ──┼─────┤  first match wins
//!   priority 10 *        ──┘     │
//!                                ▼
//!                          RuleDecision
//! ```
//!
//! # The order is total, and it is decided once
//!
//! Rules are consulted in order of **priority ascending, then id
//! ascending**, and the first enabled rule whose matcher accepts the request
//! produces the decision. Nothing after it is consulted, so two decisions
//! never both apply and there is no question of combining them.
//!
//! # Transformations are consulted on the way
//!
//! A Modify Request or Modify Response rule decides nothing, so it does not
//! end the walk. One that matches is collected and the rules below it are
//! still consulted, until the first decision — which ends the walk as it
//! always has. So:
//!
//! ```text
//! priority 0   Modify Request   add Authorization   ──▶ collected
//! priority 5   Allow            /health             ──▶ (did not match)
//! priority 10  Map Remote       api.example.com     ──▶ the decision; walk ends
//! priority 20  Modify Request   add X-Debug         ──▶ never consulted
//! ```
//!
//! A decision above a transformation therefore exempts what it matches from
//! it — an `Allow` at the top of the list is how a transformation is kept off
//! one request — and a transformation below the deciding rule is not applied,
//! because nothing below the deciding rule is ever consulted.
//!
//! Every rule is matched against the request as it arrived, not as an
//! earlier transformation left it: a transformation cannot make a request
//! match a rule it did not match, and the walk is one pass over one request.
//!
//! Where two transformations change the same thing, **the one higher in the
//! list wins**, exactly as it would were they decisions. [`RuleOutcome`]
//! gives them in the order they are to be applied, which is therefore the
//! walk's order reversed: the rule consulted first is applied last.
//!
//! A rule set with no transformations in it is evaluated exactly as before.
//!
//! Priority is the part a person chooses; the id breaks the tie, and breaks
//! it the same way for ever, because a [`RuleId`](crate::RuleId) is a
//! time-ordered UUID minted when the rule was created. So "equal priority,
//! older rule first" needs no sequence column and cannot disagree with one.
//!
//! A [`RuleSet`] is sorted when it is built, not when it is consulted, so
//! evaluating a request is a scan of a slice in a fixed order. Nothing here
//! iterates a hash map, and nothing depends on the order rows came back from
//! a database in.
//!
//! # Evaluation touches nothing
//!
//! [`RuleSet::evaluate`] takes `&self` and a borrowed request, and returns a
//! decision. No file is opened, no socket is opened, no lock is taken, and
//! nothing is written — including for a `MapLocal`, whose file is read by
//! the pipeline afterwards and only if the decision is acted on.
//!
//! # Changing the rules under a running proxy
//!
//! [`RuleEngine`] holds one shared snapshot. A request takes a cheap clone of
//! it and evaluates against that, holding no lock while it does; a CRUD
//! operation builds a whole new snapshot and swaps it in. So a rule change
//! never appears half-applied to a request, and a request in flight finishes
//! against the rules it started with.

use std::sync::{Arc, RwLock};

use crate::action::{BlockAction, MapLocal, MapRemote, Redirect};
use crate::matcher::RequestContext;
use crate::modify::{ModifyRequest, ModifyResponse};
use crate::rule::{Rule, RuleId};
use crate::RuleAction;

/// What the rules say should happen to a request.
///
/// Carries the rule that decided it, so the pipeline can report which rule
/// answered a request without looking it up again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleDecision {
    /// No rule matched, or the one that did says to carry the request.
    Allow {
        /// The rule that allowed it, or `None` if none matched at all.
        rule: Option<RuleId>,
    },
    /// Answer the request here.
    Block {
        /// The rule that blocked it.
        rule: RuleId,
        /// How to answer.
        block: BlockAction,
    },
    /// Answer the request with a file.
    MapLocal {
        /// The rule that mapped it.
        rule: RuleId,
        /// Which file, and how to answer with it.
        map: MapLocal,
    },
    /// Send the request somewhere else.
    MapRemote {
        /// The rule that mapped it.
        rule: RuleId,
        /// Where it goes instead.
        map: MapRemote,
    },
    /// Answer the request with a redirect.
    Redirect {
        /// The rule that redirected it.
        rule: RuleId,
        /// Where to, and with which status.
        redirect: Redirect,
    },
}

impl RuleDecision {
    /// The decision for a request no rule matched.
    pub fn allowed() -> Self {
        Self::Allow { rule: None }
    }

    /// Which rule decided this, if one did.
    pub fn rule(&self) -> Option<RuleId> {
        match self {
            Self::Allow { rule } => *rule,
            Self::Block { rule, .. }
            | Self::MapLocal { rule, .. }
            | Self::MapRemote { rule, .. }
            | Self::Redirect { rule, .. } => Some(*rule),
        }
    }

    /// What this decision is called, for a log line.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Allow { .. } => "allow",
            Self::Block { .. } => "block",
            Self::MapLocal { .. } => "map_local",
            Self::MapRemote { .. } => "map_remote",
            Self::Redirect { .. } => "redirect",
        }
    }

    /// Whether this decision lets the request go to its original destination.
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }

    /// Whether this decision sends the request to an origin at all — its
    /// own, or the one a mapping names — rather than answering it here.
    pub fn is_carried(&self) -> bool {
        matches!(self, Self::Allow { .. } | Self::MapRemote { .. })
    }
}

/// One transformation, and the rule it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transformation<T> {
    /// The rule that asked for it.
    pub rule: RuleId,
    /// What it changes.
    pub change: T,
}

/// Everything the rules say about one request: the decision, and the
/// transformations collected on the way to it.
///
/// Both lists are in the order they are to be applied — lowest in the rule
/// list first, so that the rule nearest the top has the last word. See the
/// module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleOutcome {
    /// What happens to the request.
    pub decision: RuleDecision,
    /// What happens to the request on its way out, if it goes out.
    pub request: Vec<Transformation<ModifyRequest>>,
    /// What happens to the origin's answer on its way back, if there is one.
    pub response: Vec<Transformation<ModifyResponse>>,
}

impl RuleOutcome {
    /// The outcome for a request no rule matched.
    pub fn allowed() -> Self {
        Self {
            decision: RuleDecision::allowed(),
            request: Vec::new(),
            response: Vec::new(),
        }
    }
}

/// The rules, in the order they are consulted.
///
/// Immutable. Building one sorts it; evaluating one only reads it. A rule
/// set is shared between every request being served at a moment, which is
/// what makes it cheap to hand out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

impl RuleSet {
    /// A rule set with no rules in it, which allows everything.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The rules, put into the order they are consulted in.
    ///
    /// Sorting happens here, once, rather than at every request. Disabled
    /// rules are kept — a rule set is the whole of what is configured, and
    /// evaluation skips them — so that turning one on is a change to one
    /// field rather than a different set.
    pub fn new(rules: impl IntoIterator<Item = Rule>) -> Self {
        let mut rules: Vec<Rule> = rules.into_iter().collect();
        // Total, and stable across processes: priority is what a person
        // chose, and the id breaks the tie by when the rule was created.
        rules.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.id.cmp(&right.id))
        });

        Self { rules }
    }

    /// The rules, in the order they are consulted.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// How many rules there are, enabled or not.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether there are no rules at all.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// How many rules are enabled.
    pub fn enabled_count(&self) -> usize {
        self.rules.iter().filter(|rule| rule.enabled).count()
    }

    /// What should happen to `request`.
    ///
    /// The first enabled decision that matches decides, and nothing after it
    /// is consulted. Transformations are passed over: they decide nothing.
    /// [`outcome`](Self::outcome) is the same walk with them collected. Pure:
    /// see the module documentation.
    pub fn evaluate(&self, request: &RequestContext<'_>) -> RuleDecision {
        self.outcome(request).decision
    }

    /// The decision for `request`, and the transformations above it.
    pub fn outcome(&self, request: &RequestContext<'_>) -> RuleOutcome {
        let mut outcome = RuleOutcome::allowed();

        for rule in &self.rules {
            if !rule.enabled || !rule.matcher.matches(request) {
                continue;
            }

            let decision = match &rule.action {
                RuleAction::ModifyRequest(change) => {
                    outcome.request.push(Transformation {
                        rule: rule.id,
                        change: change.clone(),
                    });
                    continue;
                }
                RuleAction::ModifyResponse(change) => {
                    outcome.response.push(Transformation {
                        rule: rule.id,
                        change: change.clone(),
                    });
                    continue;
                }
                RuleAction::Allow => RuleDecision::Allow {
                    rule: Some(rule.id),
                },
                RuleAction::Block(block) => RuleDecision::Block {
                    rule: rule.id,
                    block: block.clone(),
                },
                RuleAction::MapLocal(map) => RuleDecision::MapLocal {
                    rule: rule.id,
                    map: map.clone(),
                },
                RuleAction::MapRemote(map) => RuleDecision::MapRemote {
                    rule: rule.id,
                    map: map.clone(),
                },
                RuleAction::Redirect(redirect) => RuleDecision::Redirect {
                    rule: rule.id,
                    redirect: redirect.clone(),
                },
            };

            outcome.decision = decision;
            break;
        }

        // The walk found them highest first; the highest is applied last, so
        // that it is the one that wins where two change the same thing.
        outcome.request.reverse();
        outcome.response.reverse();

        outcome
    }
}

/// The rule set a running proxy is using.
///
/// One of these exists per process. Requests read from it; rule changes
/// replace what it holds.
///
/// The lock is held only long enough to clone an `Arc` — never while a
/// request is evaluated, and never while the database is touched. A reader
/// that has taken a snapshot is unaffected by a writer replacing it, so a
/// rule change cannot corrupt an evaluation in progress or be seen half-done.
#[derive(Debug)]
pub struct RuleEngine {
    rules: RwLock<Arc<RuleSet>>,
}

impl RuleEngine {
    /// An engine with no rules, which allows everything.
    pub fn new() -> Self {
        Self::with(RuleSet::empty())
    }

    /// An engine serving `rules`.
    pub fn with(rules: RuleSet) -> Self {
        Self {
            rules: RwLock::new(Arc::new(rules)),
        }
    }

    /// The rules as they are right now.
    ///
    /// Cheap: one `Arc` clone. Evaluate against what this returns rather
    /// than calling [`evaluate`](Self::evaluate) repeatedly if a request
    /// needs the same rules twice — the two calls could otherwise straddle
    /// an update.
    pub fn snapshot(&self) -> Arc<RuleSet> {
        // A writer that panicked while replacing the snapshot would poison
        // this lock. The rules it left are still a valid set to serve, and a
        // proxy that stops carrying traffic because a rule edit failed is
        // worse than one that carries it under the rules it already had.
        match self.rules.read() {
            Ok(rules) => Arc::clone(&rules),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    /// Replaces the rules with `rules`.
    ///
    /// Requests already being evaluated finish against the rules they
    /// started with; every request after this sees the new set.
    pub fn replace(&self, rules: RuleSet) {
        let rules = Arc::new(rules);

        match self.rules.write() {
            Ok(mut current) => *current = rules,
            Err(poisoned) => *poisoned.into_inner() = rules,
        }
    }

    /// What should happen to `request`, under the rules as they are now.
    pub fn evaluate(&self, request: &RequestContext<'_>) -> RuleDecision {
        self.snapshot().evaluate(request)
    }

    /// The decision and the transformations for `request`, under the rules
    /// as they are now — from one snapshot, so the two cannot disagree.
    pub fn outcome(&self, request: &RequestContext<'_>) -> RuleOutcome {
        self.snapshot().outcome(request)
    }
}

impl Default for RuleEngine {
    fn default() -> Self {
        Self::new()
    }
}
