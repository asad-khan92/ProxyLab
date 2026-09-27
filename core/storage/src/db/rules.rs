//! Keeping rules, so they outlive the process that was told about them.
//!
//! ```text
//! Rule  ──serialize──▶  one row in `rules`  ──deserialize──▶  Rule
//!                                            validated on the way back
//! ```
//!
//! A rule is written by a person and then consulted for every matching
//! request, so it is worth more care coming out of the database than a
//! captured exchange is. Everything read back goes through
//! [`Rule::validate`]: a row whose JSON was edited by hand, or written by a
//! build that knew an action this one does not, is reported as unreadable
//! rather than handed to the engine.
//!
//! # The order is the database's
//!
//! [`Storage::list_rules`] returns rules in the order they are consulted —
//! `priority ASC, id ASC` — which is what the `rules_order` index is on. So
//! the rule set a proxy loads at startup arrives already in order.
//!
//! # Nothing here is on the request path
//!
//! These are CRUD operations: called when a person changes a rule, and once
//! when the proxy starts. A request never reaches this module — it is
//! evaluated against the in-memory snapshot `rules::RuleEngine` holds — so
//! no request ever waits for SQLite to decide what happens to it.

use rusqlite::{OptionalExtension, Row};

use rules::{Rule, RuleId, RuleUpdate};

use super::error::StorageError;
use super::store::Storage;

/// The columns of a rule row, in the order [`read_rule`] expects them.
const RULE_COLUMNS: &str =
    "id, enabled, name, priority, matcher, action, created_at_ms, updated_at_ms";

/// Stores one rule.
const INSERT_RULE: &str = "\
    INSERT INTO rules (
        id, enabled, name, priority, matcher, action, created_at_ms, updated_at_ms
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

/// Replaces the parts of one rule that can change.
///
/// Neither the id nor `created_at_ms` is among them: a rule keeps its
/// identity and the moment it was created for as long as it exists.
const UPDATE_RULE: &str = "\
    UPDATE rules SET
        enabled = ?2, name = ?3, priority = ?4, matcher = ?5, action = ?6, updated_at_ms = ?7
    WHERE id = ?1";

impl Storage {
    /// Stores a new rule.
    ///
    /// Fails if a rule with that id is already stored — an id identifies one
    /// rule — and takes nothing away from the one already there.
    pub fn insert_rule(&self, rule: &Rule) -> Result<(), StorageError> {
        let row = encode(rule)?;

        self.with_connection(|connection| {
            connection.execute(INSERT_RULE, rusqlite::params_from_iter(row))?;

            Ok(())
        })
    }

    /// Reads one rule back, or `None` if there is no such rule.
    pub fn rule(&self, id: RuleId) -> Result<Option<Rule>, StorageError> {
        let sql = format!("SELECT {RULE_COLUMNS} FROM rules WHERE id = ?1");

        let stored = self.with_connection(|connection| {
            let stored = connection
                .query_row(&sql, [id.as_text()], read_rule)
                .optional()?;

            Ok(stored)
        })?;

        stored.map(decode).transpose()
    }

    /// Every rule, in the order they are consulted in.
    ///
    /// `priority ASC, id ASC`: what a person chose, then the moment the rule
    /// was created. Disabled rules are included — the engine skips them, and
    /// a list a person is shown is the whole of what is configured.
    pub fn list_rules(&self) -> Result<Vec<Rule>, StorageError> {
        let sql = format!("SELECT {RULE_COLUMNS} FROM rules ORDER BY priority ASC, id ASC");

        let stored = self.with_connection(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement
                .query_map([], read_rule)?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(rows)
        })?;

        stored.into_iter().map(decode).collect()
    }

    /// How many rules are stored.
    pub fn count_rules(&self) -> Result<u64, StorageError> {
        self.with_connection(|connection| {
            let count: i64 =
                connection.query_row("SELECT COUNT(*) FROM rules", [], |row| row.get(0))?;

            Ok(count.max(0) as u64)
        })
    }

    /// Applies `update` to the rule `id` names, and returns what it became.
    ///
    /// `None` if there is no such rule. Read, applied, and written in one
    /// transaction, so two updates arriving together cannot leave a rule
    /// that is half of each.
    pub fn update_rule(
        &self,
        id: RuleId,
        update: RuleUpdate,
    ) -> Result<Option<Rule>, StorageError> {
        let select = format!("SELECT {RULE_COLUMNS} FROM rules WHERE id = ?1");

        self.with_connection(|connection| {
            let transaction = connection.transaction()?;

            let stored = transaction
                .query_row(&select, [id.as_text()], read_rule)
                .optional()?;
            let Some(stored) = stored else {
                return Ok(None);
            };

            // A rule that cannot be read back cannot be updated either: the
            // update would be applied to a rule this build does not
            // understand, and written back over the one that is there.
            let updated = decode(stored)?
                .updated(update)
                .map_err(|_| unreadable("action"))?;
            transaction.execute(
                UPDATE_RULE,
                rusqlite::params_from_iter(encode_update(&updated)?),
            )?;
            transaction.commit()?;

            Ok(Some(updated))
        })
    }

    /// Removes one rule, and says whether there was one to remove.
    pub fn delete_rule(&self, id: RuleId) -> Result<bool, StorageError> {
        self.with_connection(|connection| {
            let removed = connection.execute("DELETE FROM rules WHERE id = ?1", [id.as_text()])?;

            Ok(removed > 0)
        })
    }

    /// Removes every rule, and says how many there were.
    pub fn clear_rules(&self) -> Result<u64, StorageError> {
        self.with_connection(|connection| {
            let removed = connection.execute("DELETE FROM rules", [])?;

            Ok(removed as u64)
        })
    }
}

/// A rule row as the database holds it, before anything is made of it.
///
/// The JSON is still text here on purpose: turning it into a matcher and an
/// action inside a `rusqlite` row reader would report a rule this build
/// cannot read as a database error, which is not what it is.
struct StoredRule {
    id: String,
    enabled: i64,
    name: String,
    priority: i64,
    matcher: String,
    action: String,
    created_at_ms: i64,
    updated_at_ms: i64,
}

/// Reads one row into the strings and numbers it holds.
fn read_rule(row: &Row<'_>) -> rusqlite::Result<StoredRule> {
    Ok(StoredRule {
        id: row.get(0)?,
        enabled: row.get(1)?,
        name: row.get(2)?,
        priority: row.get(3)?,
        matcher: row.get(4)?,
        action: row.get(5)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
    })
}

/// The values an insert binds, in the statement's order.
fn encode(rule: &Rule) -> Result<Vec<rusqlite::types::Value>, StorageError> {
    use rusqlite::types::Value;

    // Checked on the way in as well as on the way out: a rule the engine
    // could not act on is not one worth storing.
    rule.validate().map_err(|_| unreadable("action"))?;

    let matcher = serde_json::to_string(&rule.matcher).map_err(|_| unreadable("matcher"))?;
    let action = serde_json::to_string(&rule.action).map_err(|_| unreadable("action"))?;

    Ok(vec![
        Value::Text(rule.id.as_text()),
        Value::Integer(i64::from(rule.enabled)),
        Value::Text(rule.name.clone()),
        Value::Integer(i64::from(rule.priority)),
        Value::Text(matcher),
        Value::Text(action),
        Value::Integer(rule.created_at_ms as i64),
        Value::Integer(rule.updated_at_ms as i64),
    ])
}

/// The values an update binds, in the statement's order.
///
/// The same as an insert's without `created_at_ms`, which an update does not
/// touch.
fn encode_update(rule: &Rule) -> Result<Vec<rusqlite::types::Value>, StorageError> {
    let mut values = encode(rule)?;
    // Drop `created_at_ms`, which sits between the action and the update
    // time in the insert's order.
    values.remove(6);

    Ok(values)
}

/// Turns a row back into a rule, or says it is not one.
///
/// Every part is checked: the id has to be an id, the priority has to fit in
/// one, the JSON has to parse, and what it comes to has to be a rule the
/// engine could act on.
fn decode(stored: StoredRule) -> Result<Rule, StorageError> {
    let rule = Rule {
        id: stored.id.parse().map_err(|_| unreadable("id"))?,
        enabled: stored.enabled != 0,
        name: stored.name,
        matcher: serde_json::from_str(&stored.matcher).map_err(|_| unreadable("matcher"))?,
        action: serde_json::from_str(&stored.action).map_err(|_| unreadable("action"))?,
        priority: i32::try_from(stored.priority).map_err(|_| unreadable("priority"))?,
        created_at_ms: stored.created_at_ms.max(0) as u64,
        updated_at_ms: stored.updated_at_ms.max(0) as u64,
    };
    rule.validate().map_err(|_| unreadable("action"))?;

    Ok(rule)
}

/// A rule row this build cannot make sense of.
fn unreadable(column: &'static str) -> StorageError {
    StorageError::Unreadable { column }
}
