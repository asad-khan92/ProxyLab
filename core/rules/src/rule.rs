//! One rule: what it matches, what it does, and where it sits.
//!
//! ```text
//! Rule
//! ├── id          a UUIDv7, durable and ordered by when it was minted
//! ├── enabled     a disabled rule is never consulted
//! ├── name        what a person calls it
//! ├── matcher     which requests it is for
//! ├── action      what it does with them
//! ├── priority    lower numbers are consulted first
//! └── timestamps  when it was created, and last changed
//! ```
//!
//! # Why the id is a UUIDv7
//!
//! The same two reasons `ExchangeId` is one. It is unique across runs, so a
//! rule keeps its identity across restarts and cannot collide with one
//! created later. And it is ordered by the moment it was minted, which is
//! what makes "the rule created first wins" a tie-break the database can
//! answer with `ORDER BY id` rather than a separate sequence column that
//! could disagree with it.
//!
//! # Timestamps
//!
//! Milliseconds since the Unix epoch, as a plain integer. Not
//! `proxy-storage`'s `Timestamp`: this crate depends on nothing of
//! ProxyLab's, and coupling the rule engine to the traffic-capture model to
//! borrow one struct would buy precision a rule does not use.

use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::action::{RuleAction, RuleActionError};
use crate::matcher::RuleMatcher;

/// The longest name a rule may carry.
///
/// A name is a label in a list, not a description. Long enough for any of
/// those, short enough that it is never a place to put something else.
pub const MAX_RULE_NAME_LENGTH: usize = 200;

/// Identifies one rule, for as long as the rule is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(Uuid);

impl RuleId {
    /// A new id, for a rule being created now.
    pub fn next() -> Self {
        Self(Uuid::now_v7())
    }

    /// The id as the UUID it is.
    pub fn as_uuid(self) -> Uuid {
        self.0
    }

    /// The id in the form it is written down as: hyphenated and lowercase.
    pub fn as_text(self) -> String {
        self.to_string()
    }
}

impl From<Uuid> for RuleId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl FromStr for RuleId {
    type Err = uuid::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(text).map(Self)
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.as_hyphenated().fmt(f)
    }
}

/// One rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Which rule this is.
    pub id: RuleId,
    /// Whether it is consulted at all.
    pub enabled: bool,
    /// What a person calls it.
    pub name: String,
    /// Which requests it is for.
    pub matcher: RuleMatcher,
    /// What it does with them.
    pub action: RuleAction,
    /// Where it sits in the order rules are consulted in: lower first.
    pub priority: i32,
    /// When it was created, in milliseconds since the Unix epoch.
    pub created_at_ms: u64,
    /// When it was last changed, in milliseconds since the Unix epoch.
    pub updated_at_ms: u64,
}

impl Rule {
    /// Creates a rule from `new`, minting its id and its timestamps.
    pub fn create(new: NewRule) -> Result<Self, RuleError> {
        let now = now_ms();
        let rule = Self {
            id: RuleId::next(),
            enabled: new.enabled,
            name: new.name,
            matcher: new.matcher,
            action: new.action,
            priority: new.priority,
            created_at_ms: now,
            updated_at_ms: now,
        };
        rule.validate()?;

        Ok(rule)
    }

    /// The same rule with `update` applied, and its `updated_at` moved on.
    ///
    /// A field the update does not name is left as it was, so changing a
    /// rule's priority does not require resending its matcher.
    pub fn updated(&self, update: RuleUpdate) -> Result<Self, RuleError> {
        let mut rule = self.clone();

        if let Some(enabled) = update.enabled {
            rule.enabled = enabled;
        }
        if let Some(name) = update.name {
            rule.name = name;
        }
        if let Some(matcher) = update.matcher {
            rule.matcher = matcher;
        }
        if let Some(action) = update.action {
            rule.action = action;
        }
        if let Some(priority) = update.priority {
            rule.priority = priority;
        }
        rule.updated_at_ms = now_ms();
        rule.validate()?;

        Ok(rule)
    }

    /// Whether this rule is one the engine can be given.
    ///
    /// Checked when a rule is created, when it is updated, and again when it
    /// is read back from the database: a row edited by hand cannot put a
    /// matcher that matches nothing or an action the pipeline has no answer
    /// for into a running proxy.
    pub fn validate(&self) -> Result<(), RuleError> {
        if self.name.trim().is_empty() {
            return Err(RuleError::NameEmpty);
        }
        if self.name.chars().count() > MAX_RULE_NAME_LENGTH {
            return Err(RuleError::NameTooLong);
        }
        if self.name.chars().any(char::is_control) {
            return Err(RuleError::NameInvalid);
        }
        if !self.matcher.is_valid() {
            return Err(RuleError::Matcher);
        }
        self.action.validate().map_err(RuleError::Action)?;

        Ok(())
    }

    /// The one-line form that is safe to write to a log.
    ///
    /// The id, whether it is on, its priority, and what it does. Not its
    /// name, which is text a person typed, and not its matcher, which names
    /// hosts and paths a developer is working on.
    pub fn summary(&self) -> String {
        format!(
            "{} priority {} {} ({})",
            self.id,
            self.priority,
            self.action.name(),
            if self.enabled { "enabled" } else { "disabled" }
        )
    }
}

/// A rule that has not been created yet.
///
/// What a client sends to create one: everything but the id and the
/// timestamps, which are the server's to mint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewRule {
    /// Whether the rule is consulted once it exists.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// What a person calls it.
    pub name: String,
    /// Which requests it is for.
    #[serde(default)]
    pub matcher: RuleMatcher,
    /// What it does with them.
    pub action: RuleAction,
    /// Where it sits in the order. Lower numbers are consulted first.
    #[serde(default)]
    pub priority: i32,
}

fn enabled_by_default() -> bool {
    true
}

impl NewRule {
    /// A rule called `name` that does `action` to what `matcher` describes.
    pub fn new(name: impl Into<String>, matcher: RuleMatcher, action: RuleAction) -> Self {
        Self {
            enabled: true,
            name: name.into(),
            matcher,
            action,
            priority: 0,
        }
    }

    /// The same rule, at `priority`.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// The same rule, disabled to start with.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// A change to a rule that already exists.
///
/// Every field is optional, and one that is not named is not changed. That
/// is what lets "turn this rule off" be an update carrying one field rather
/// than the whole rule read back, changed, and sent again — which would race
/// with anything else editing it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleUpdate {
    /// Whether the rule is consulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// What a person calls it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Which requests it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<RuleMatcher>,
    /// What it does with them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<RuleAction>,
    /// Where it sits in the order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
}

impl RuleUpdate {
    /// An update that changes nothing.
    pub fn none() -> Self {
        Self::default()
    }

    /// An update that only turns a rule on or off.
    pub fn enabled(enabled: bool) -> Self {
        Self {
            enabled: Some(enabled),
            ..Self::default()
        }
    }

    /// Whether this update changes anything at all.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Why a rule is not one this build can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleError {
    /// The rule has no name.
    NameEmpty,
    /// The name is longer than [`MAX_RULE_NAME_LENGTH`].
    NameTooLong,
    /// The name carries characters a name cannot.
    NameInvalid,
    /// A matching criterion was set to something that matches nothing.
    Matcher,
    /// The action is not one this build can carry out.
    Action(RuleActionError),
}

impl RuleError {
    /// What went wrong, in words that name nothing the caller supplied.
    pub fn reason(self) -> &'static str {
        match self {
            Self::NameEmpty => "a rule must have a name",
            Self::NameTooLong => "a rule name must be at most 200 characters",
            Self::NameInvalid => "a rule name must not contain control characters",
            Self::Matcher => "a rule's matching criteria are not ones that could match",
            Self::Action(error) => error.reason(),
        }
    }
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for RuleError {}

/// Now, in milliseconds since the Unix epoch.
///
/// A clock set before 1970 becomes the epoch rather than an error: a
/// nonsensical clock is not a reason to refuse to create a rule.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}
