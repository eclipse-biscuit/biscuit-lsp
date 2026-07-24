/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

//! This module encodes knowledge about datalog’s AST. Most of the logic is about
//! accomodating partial ASTs encountered where the user is typing code.

use ropey::Rope;

use crate::tree_sitter;

/// Get the semantic node type at the cursor position.
///
/// Direct AST read: find the node, and if it's punctuation, report its parent
/// instead (nobody wants a hover tooltip that just says `";"`). No further
/// disambiguation — that complexity belongs to `detect_valid_insertions`.
pub fn detect_node_type(tree: &tree_sitter::Tree, offset: usize) -> &'static str {
    let node = tree_sitter::find_node_at_cursor(tree.root_node(), offset);
    if is_punctuation(node.kind()) {
        if let Some(parent) = node.parent() {
            return parent.kind();
        }
    }
    node.kind()
}

/// A distinct "thing that can be typed" at a cursor position.
///
/// Mostly mirrors tree-sitter's own node kinds, with a focus on what is actually
/// typed: `nname` does not tell us whether we want to type a method name or a predicate name,
/// and wrapper nodes are directly expanded to the first token of each alternative
/// (eg we’re not interested in inserting a fact, rather a predicate name)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Insertion {
    PredicateName,
    MethodName,
    ParamName,
    Check,
    Policy,
    OriginClause,
    Term,
    FactTerm,
    SetTerm,
    Variable,
    OriginElement,
}

/// Detect the [`Insertion`]s that are valid at the cursor.
///
/// Empty `Vec` means "no recovery leg matched" — a deliberately punted case,
/// not a crash. See the module doc for the shape of what this does and does
/// not attempt to recover from.
pub fn detect_valid_insertions(
    tree: &tree_sitter::Tree,
    rope: &Rope,
    offset: usize,
) -> Vec<Insertion> {
    let root = tree.root_node();
    let node = root
        .descendant_for_byte_range(offset, offset)
        .unwrap_or(root);

    match (
        find_seed(node, offset),
        recover_orphaned_trailer(node, rope),
        recover_trailing_variable(node),
        node.parent(),
    ) {
        (Some(seed), _, _, _) => resolve_seed(seed),
        (None, Some(contexts), _, _) => contexts,
        (None, None, Some(contexts), _) => contexts,
        // children_in_context handles an ERROR parent itself (see
        // recover_error_keyword_prefix), so no separate "parent is ERROR:
        // deferred" arm is needed here — it falls back to empty on its own.
        (None, None, None, Some(parent)) => children_in_context(node, parent),
        (None, None, None, None) => trailing_error_prev_char(root, rope, offset)
            .map(|ctx| vec![ctx])
            .unwrap_or_else(|| possible_children(node.kind())),
    }
}

/// A node kind is punctuation if it's not made of identifier-like characters —
/// grammar rule names and keywords are alphanumeric/underscore (`nname`,
/// `rule_body`, `trusting`, `true`, `check if`), while syntax tokens are made
/// of symbol characters (`(`, `;`, `<-`, `!=`). Cheaper and more robust than
/// hand-maintaining an exhaustive list of every punctuation token.
fn is_punctuation(kind: &str) -> bool {
    !kind
        .chars()
        .all(|c| c.is_alphanumeric() || c.is_whitespace() || c == '_')
}

/// The node whose grammar position `detect_valid_insertions` should report on,
/// found one of two structurally different ways — see [`find_seed`].
enum Seed<'a> {
    /// `node` sits exactly at its own start; real content, safe to climb via
    /// `Node::parent()`.
    Direct(::tree_sitter::Node<'a>),
    /// Already the outermost node of a synthesized `MISSING` chain (see
    /// `missing_chain_representative`) — final answer, no further climbing.
    /// `Node::parent()` must never be called on a zero-width node (see that
    /// function's doc comment), so this variant is kept separate from
    /// `Direct` specifically to stop `resolve_seed` from trying to.
    Recovered(::tree_sitter::Node<'a>),
}

/// Find the node whose grammar position we should actually report on.
///
/// Two ways in:
/// - `node` sits exactly at its own start (nothing of it typed yet) and isn't
///   bare punctuation — e.g. cursor at the start of an `nname`, or at the start
///   of a `true` the user genuinely wrote. Punctuation is excluded because
///   every single-byte token trivially satisfies "offset == start", which
///   would hijack cases that should fall through to `children_in_context` or
///   `recover_orphaned_trailer` instead.
/// - `node` has no direct hit at its own start, but its previous sibling's
///   right-spine bottoms out at a zero-width `MISSING` node tree-sitter
///   synthesized during recovery (see `missing_chain_representative`).
fn find_seed(node: ::tree_sitter::Node, offset: usize) -> Option<Seed> {
    if offset == node.start_byte() && !is_punctuation(node.kind()) {
        Some(Seed::Direct(node))
    } else {
        missing_chain_representative(node).map(Seed::Recovered)
    }
}

/// `descendant_for_byte_range` can never return a zero-width node (its
/// containment check requires the query to end strictly before the node's
/// end byte, which a zero-width node can never satisfy). So when the grammar
/// synthesizes a `MISSING` placeholder chain for an empty slot, the only way
/// to reach it is to step to the previous sibling of whatever real node the
/// query *did* return, then repeatedly follow the last child.
///
/// That descent has to stop the moment it *becomes* zero-width, rather than
/// continuing on to the deepest leaf: `Node::parent()` has the exact same
/// blind spot as `descendant_for_byte_range` (its internal position-based walk
/// can't locate a zero-width node either), so calling `.parent()` on e.g. the
/// deepest `MISSING` leaf doesn't return its structural parent — it returns an
/// unrelated preceding token. Since `.child()` is unaffected (it's a plain
/// structural lookup), the fix is to find the *outermost* zero-width node on
/// the way down, and never call `.parent()` on anything in this chain at all.
fn missing_chain_representative(node: ::tree_sitter::Node) -> Option<::tree_sitter::Node> {
    let mut candidate = node.prev_sibling()?;
    // Descend past real content only; stop the instant it becomes zero-width
    // (that's the outermost node of the chain) or we run out of children.
    while candidate.start_byte() != candidate.end_byte() && candidate.child_count() > 0 {
        candidate = candidate.child(candidate.child_count() - 1)?;
    }
    (candidate.start_byte() == candidate.end_byte()).then_some(candidate)
}

/// From a direct-hit seed, climb through parents while the parent's start
/// byte stays identical to the seed's. Being at a node's start means being,
/// simultaneously, at the start of every ancestor that begins at that same
/// byte — so the outermost such ancestor is the one whose grammar position
/// (and thus `possible_children`) is actually the answer. This is what
/// collapses e.g. `expression -> term -> boolean -> true` (all four starting
/// at the same byte) down to whichever of them is the real "choice" point
/// (`rule_body`, `term`, ...). Only ever called on real, non-zero-width nodes
/// reached via `find_seed`'s direct branch — see `Seed` and
/// `missing_chain_representative` for why `.parent()` isn't safe otherwise.
fn climb_same_start(seed: ::tree_sitter::Node) -> ::tree_sitter::Node {
    let start = seed.start_byte();
    let mut representative = seed;
    while let Some(parent) = representative.parent() {
        if parent.start_byte() != start {
            break;
        }
        representative = parent;
    }
    representative
}

fn resolve_seed(seed: Seed) -> Vec<Insertion> {
    match seed {
        Seed::Direct(node) => {
            let representative = climb_same_start(node);
            match (representative.id() != node.id(), node.parent()) {
                // Climbing advanced at least once: representative is the real choice point.
                (true, _) => possible_children(representative.kind()),
                // Climbing didn't move (e.g. seed is a fixed-position token like
                // the "(" right after a name): fall back to position-aware handling.
                (false, Some(parent)) => children_in_context(node, parent),
                (false, None) => possible_children(node.kind()),
            }
        }
        Seed::Recovered(representative) => possible_children(representative.kind()),
    }
}

/// Recover from the third parse-failure shape: the grammar orphans a trailing
/// token into its own small, real-width `ERROR` node immediately after an
/// otherwise fully-parsed sibling, rather than synthesizing a `MISSING` chain
/// (contrast `missing_chain_seed`) or swallowing the whole statement (contrast
/// the `trailing_error_prev_char` fallback). Seen so far for a dangling
/// `trusting` keyword or a dangling `,` continuing either an `origin_clause`
/// or an ordinary `rule_body` list.
fn recover_orphaned_trailer(node: ::tree_sitter::Node, rope: &Rope) -> Option<Vec<Insertion>> {
    let prev = node.prev_sibling()?;
    if !prev.is_error() || prev.start_byte() == prev.end_byte() {
        return None;
    }

    let text = rope
        .byte_slice(prev.start_byte()..prev.end_byte())
        .to_string();
    match text.trim() {
        "trusting" => Some(vec![Insertion::OriginElement]),
        "," => {
            // Only a rule/check/policy sibling means this comma sits at the
            // authorizer_element level (continuing that statement's rule_body
            // or origin_clause) rather than inside e.g. a predicate's argument
            // list, where the plain fallback already gives the right answer.
            let statement = prev.prev_sibling()?;
            if !matches!(statement.kind(), "rule" | "check" | "policy") {
                return None;
            }
            let rule_body = last_child(statement)?;
            let continues_origin_clause =
                last_child(rule_body).map(|n| n.kind()) == Some("origin_clause");
            Some(if continues_origin_clause {
                vec![Insertion::OriginElement]
            } else {
                possible_children("rule_body")
            })
        }
        _ => None,
    }
}

/// Recover the "cursor right after a complete variable that's the entire
/// rule_body content, before the terminating punctuation" case — e.g.
/// `foo($bar) <- $|;`. Unlike `recover_orphaned_trailer`, the preceding
/// statement here is real and complete (no `ERROR` involved at all): the
/// variable was actually typed, but the cursor sits exactly at its end byte,
/// which excludes it from `descendant_for_byte_range` (see the module's
/// general "exclusive end" note) and it isn't zero-width either, so
/// `missing_chain_representative` doesn't apply. Nothing else currently
/// recognizes "you might still be extending this variable's name."
fn recover_trailing_variable(node: ::tree_sitter::Node) -> Option<Vec<Insertion>> {
    let statement = node.prev_sibling()?;
    if !matches!(statement.kind(), "rule" | "check" | "policy") {
        return None;
    }
    let mut candidate = statement;
    while candidate.child_count() > 0 {
        candidate = candidate.child(candidate.child_count() - 1)?;
    }
    (candidate.kind() == "$").then_some(vec![Insertion::Variable])
}

fn last_child(node: ::tree_sitter::Node) -> Option<::tree_sitter::Node> {
    let count = node.child_count();
    (count > 0).then(|| node.child(count - 1)).flatten()
}

/// Recover from the first parse-failure shape: the whole statement collapses
/// into one flat `ERROR` directly under `source_file` (e.g. `check if $x.|`),
/// leaving no structure at all to inspect. `$`/`.` are the only two characters
/// grammar-unique enough to guess from blindly. Only applies when the parse
/// actually failed (root's last child is an `ERROR`) — a clean top-level
/// position never reaches this arm's caller in the first place.
fn trailing_error_prev_char(
    root: ::tree_sitter::Node,
    rope: &Rope,
    offset: usize,
) -> Option<Insertion> {
    let count = root.child_count();
    let last = (count > 0).then(|| root.child(count - 1)).flatten()?;
    if !last.is_error() || offset == 0 {
        return None;
    }
    match rope.byte_slice((offset - 1)..offset) {
        s if s == "$" => Some(Insertion::Variable),
        s if s == "." => Some(Insertion::MethodName),
        _ => None,
    }
}

/// Position-aware handling for grammar slots where a single child kind
/// (`nname`) or a single boundary token (`(`) means something different
/// depending on which parent it sits in.
fn children_in_context(node: ::tree_sitter::Node, parent: ::tree_sitter::Node) -> Vec<Insertion> {
    match (node.kind(), parent.kind()) {
        // The name slot: fixed position, not a repeatable/alternative child.
        ("nname", "methods") => vec![Insertion::MethodName],
        ("nname", "predicate" | "fact") => vec![Insertion::PredicateName],
        ("nname", "param") => vec![], // no test coverage yet; deferred

        // Cursor right after a name, right before "(": still could be
        // extending the name (e.g. "use|r" -> "user2") rather than starting
        // the argument list — the argument list itself is reached once the
        // cursor is *past* "(", which the MISSING-chain seed already covers
        // for the empty case.
        ("(", "predicate" | "fact") => vec![Insertion::PredicateName],
        ("(", "methods") => vec![Insertion::MethodName],

        // Mid-typing (or right at) a variable: more precise than falling
        // through to possible_children("term")/("variable"), and the only
        // clean-parse path that makes Insertion::Variable reachable at all
        // (elsewhere it only comes from the flat-ERROR-blob prev-char fallback).
        ("variable", "term") => vec![Insertion::Variable],
        ("$", "variable") => vec![Insertion::Variable],

        // A flat ERROR blob is otherwise a dead end (see
        // recover_error_keyword_prefix for the one shape it's not).
        (_, "ERROR") => recover_error_keyword_prefix(parent).unwrap_or_default(),

        (_, parent_kind) => possible_children(parent_kind),
    }
}

/// Recover from a variant of the first parse-failure shape: the whole
/// statement collapses into one flat `ERROR`, but unlike
/// `trailing_error_prev_char`'s case, a *later* token inside it (e.g. an
/// `nname` typed with nothing valid after it, like `check if user` with no
/// `(`/`;`) is what the cursor's direct-hit seed lands on — so this is
/// reached through `children_in_context`, not the top-level "no seed at all"
/// fallback. If the ERROR's first child is a check/policy keyword, this is
/// really an incomplete check/policy statement, so treat it exactly like
/// being inside that statement's `rule_body`.
fn recover_error_keyword_prefix(parent: ::tree_sitter::Node) -> Option<Vec<Insertion>> {
    if !parent.is_error() {
        return None;
    }
    match parent.child(0)?.kind() {
        "check if" | "check all" | "reject if" => Some(possible_children("check")),
        "allow if" | "deny if" => Some(possible_children("policy")),
        _ => None,
    }
}

/// Grammar-driven parent -> [`Insertion`] mapping, hand-derived from
/// `tree-sitter-biscuit/grammar.js` and maintained by hand. Deliberately a
/// flat lookup table rather than a general grammar-rule engine: swap this
/// function's body if a richer source of truth (e.g. parsing `grammar.js`
/// itself) becomes worth it.
///
/// `authorizer_element` and `rule_body` are pure single-choice wrappers
/// nobody actually types, so no [`Insertion`] variant exists for either —
/// but both still need their *own* table entry (not just an inlined one at
/// `check`/`policy`/`rule`/`source_file`), since a direct-hit climb or a
/// fallback lookup can land squarely on the wrapper itself.
fn possible_children(parent_kind: &str) -> Vec<Insertion> {
    use Insertion::*;
    match parent_kind {
        "rule_body" => vec![PredicateName, Term],
        "expression" => vec![Term],
        "predicate" => vec![Term],
        "fact" => vec![FactTerm],
        "methods" => vec![Term], // MethodName only via children_in_context
        "array" => vec![FactTerm],
        "set" => vec![SetTerm],
        "parens" => vec![Term],
        "closure" => vec![Term],
        "method_argument" => vec![Term],
        // boolean/null are named wrappers around a literal keyword child
        // ("true"/"false", "null") — unlike number/string/date/bytes, which
        // are flat terminal tokens already covered by falling through to
        // "term"'s own entry directly. Mid-typing "true"/"false"/"null"
        // lands on these as the immediate parent, one level short of "term".
        "boolean" => vec![Term],
        "null" => vec![Term],
        // Identity entries: reached when climbing/fallback lands squarely on
        // one of these (e.g. the MISSING-chain climb stopping at "term" itself
        // because its real-content parent has a different start byte).
        "term" => vec![Term],
        "fact_term" => vec![FactTerm],
        "set_term" => vec![SetTerm],
        // "unary_op_expression" => vec![Expression],
        // "binary_op_expression" => vec![Expression],
        "origin_clause" => vec![OriginElement],
        "origin_element" => vec![OriginElement],
        // check/policy/rule inline rule_body's own answer directly (same as
        // rule_body's entry above) since nobody types "rule_body" itself.
        "check" => vec![PredicateName, Term],
        "policy" => vec![PredicateName, Term],
        "rule" => vec![PredicateName, Term], // predicate head is a fixed slot, not a choice
        // authorizer_element needs its own entry (unlike rule_body, it isn't
        // *only* ever inlined — a direct-hit climb can land here itself, e.g.
        // cursor right after a complete top-level statement's last token).
        "authorizer_element" => vec![PredicateName, Policy, Check],
        // source_file inlines authorizer_element's answer directly, same reasoning.
        "source_file" => vec![PredicateName, Policy, Check, OriginClause],
        _ => vec![],
    }
}

/// Biscuit language keywords with their descriptions
/// Format: (keyword, description, has_snippet)
pub const KEYWORDS: &[(&str, &str, bool)] = &[
    ("check if", "Check constraint", true),
    ("check all", "Check all constraints", true),
    ("reject if", "Reject if condition", true),
    ("allow if", "Allow if condition (policy)", true),
    ("deny if", "Deny if condition (policy)", true),
    ("trusting", "Origin clause", true),
    ("previous", "Trust previous block", false),
    ("authority", "Trust authority block", false),
    ("true", "Boolean true", false),
    ("false", "Boolean false", false),
    ("null", "Null value", false),
];
