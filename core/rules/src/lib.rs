//! The rule engine: what ProxyLab does with a request before it carries it.
//!
//! ```text
//!                        request
//!                           │
//!                           ▼
//!                     ┌───────────┐
//!                     │  RuleSet  │   by priority then id: transformations
//!                     └─────┬─────┘   collected, first decision wins
//!                           │
//!     ┌────────┬────────┬───┴─────┬───────────┬──────────┐
//!     ▼        ▼        ▼         ▼           ▼          ▼
//!   Allow    Block   MapLocal  MapRemote   Redirect   + ModifyRequest,
//!                                                       ModifyResponse
//! ```
//!
//! # Deciding is not doing
//!
//! Everything in this crate is a pure function of the rules and the request.
//! [`RuleSet::evaluate`] opens no file, makes no network request, reads no
//! database, and mutates nothing — it returns a [`RuleDecision`] and the
//! proxy pipeline carries it out. That separation is what makes the whole of
//! the matching testable without a proxy, and what keeps a misconfigured
//! `MapLocal` path from turning rule evaluation into filesystem I/O on every
//! request.
//!
//! It also means a decision is reproducible: the same rules and the same
//! request always produce the same decision, on any machine, in any order of
//! evaluation.
//!
//! # No UI, no storage, no proxy
//!
//! This crate depends on nothing of ProxyLab's. The rules it evaluates are
//! persisted by `proxy-storage`, managed over `proxy-ipc`, and executed by
//! `proxy-engine`; none of that is visible from here. A rule is a value.
//!
//! # What a rule may match on
//!
//! Host, path, and method — the three things a traffic row already shows.
//! There is no expression language, no regex, and no matching on headers,
//! queries or bodies. See [`matcher`] for the exact semantics of each, which are worth
//! reading before writing a rule: they are deliberately literal.

pub mod action;
pub mod engine;
pub mod matcher;
pub mod modify;
pub mod rule;

pub use action::{
    BlockAction, MapLocal, MapRemote, Redirect, RemoteScheme, RemoteTarget, RuleAction,
    RuleActionError, DEFAULT_BLOCK_STATUS, DEFAULT_REDIRECT_STATUS, MAX_MAP_LOCAL_BYTES,
    REDIRECT_STATUSES,
};
pub use engine::{RuleDecision, RuleEngine, RuleOutcome, RuleSet, Transformation};
pub use matcher::{HostMatch, MethodMatch, PathMatch, RequestContext, RuleMatcher};
pub use modify::{
    apply_query, BodyEdit, HeaderEdit, ModifyRequest, ModifyResponse, PathEdit, QueryEdit,
    MAX_BODY_TEXT_BYTES, MAX_EDITS, PROTECTED_HEADERS,
};
pub use rule::{NewRule, Rule, RuleError, RuleId, RuleUpdate, MAX_RULE_NAME_LENGTH};
