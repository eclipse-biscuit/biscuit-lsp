/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

//! This module encodes knowledge about datalog’s AST. Most of the logic is about
//! accomodating partial ASTs encountered where the user is typing code.

use ropey::Rope;
use std::collections::HashSet;

use crate::tree_sitter;

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
        is_trailing_variable(node),
        node.parent(),
    ) {
        (Some(seed), _, _, _) => resolve_seed(seed, root, offset),
        (None, Some(contexts), _, _) => contexts,
        (None, None, true, _) => vec![Insertion::Variable],
        (None, None, false, Some(parent)) => children_in_context(node, parent),
        (None, None, false, None) => trailing_error_prev_char(root, rope, offset)
            .map(|ctx| vec![ctx])
            .or_else(|| trailing_error_keyword(root))
            .unwrap_or_else(|| top_level_choices(root, offset)),
    }
}

fn at_document_start(root: ::tree_sitter::Node, offset: usize) -> bool {
    match root.child(0) {
        Some(first) => offset <= first.start_byte(),
        None => true,
    }
}

fn top_level_choices(root: ::tree_sitter::Node, offset: usize) -> Vec<Insertion> {
    let mut choices = possible_children("authorizer_element");
    if at_document_start(root, offset) {
        choices.push(Insertion::OriginClause);
    }
    choices
}

fn is_punctuation(kind: &str) -> bool {
    !kind
        .chars()
        .all(|c| c.is_alphanumeric() || c.is_whitespace() || c == '_')
}

/// The node whose grammar position `detect_valid_insertions` should report on
enum Seed<'a> {
    /// `node` sits exactly at its own start; real content, safe to climb via
    /// `Node::parent()`.
    Direct(::tree_sitter::Node<'a>),
    /// Already the outermost node of a synthesized `MISSING` chain
    Recovered(::tree_sitter::Node<'a>),
}

/// Find the node whose grammar position we should actually report on.
///
/// Two ways in:
/// - `node` sits exactly at its own start (nothing of it typed yet) and isn't
///   bare punctuation
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

fn resolve_seed(seed: Seed, root: ::tree_sitter::Node, offset: usize) -> Vec<Insertion> {
    match seed {
        Seed::Direct(node) => {
            let representative = climb_same_start(node);
            match (representative.id() != node.id(), node.parent()) {
                // Climbing reached source_file itself: top-level, so OriginClause
                // availability needs the direct at_document_start check too.
                (true, _) if representative.kind() == "source_file" => {
                    top_level_choices(root, offset)
                }
                // Climbing advanced at least once: representative is the real choice point.
                (true, _) => possible_children(representative.kind()),
                // Climbing didn't move (e.g. seed is a fixed-position token like
                // the "(" right after a name): fall back to position-aware handling.
                (false, Some(parent)) => children_in_context(node, parent),
                // node itself is source_file (e.g. cursor at offset 0 of an
                // otherwise-empty document): same top-level answer.
                (false, None) => top_level_choices(root, offset),
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

fn is_trailing_variable(node: ::tree_sitter::Node) -> bool {
    ((|| {
        let mut candidate = node.prev_sibling()?;
        while candidate.child_count() > 0 {
            candidate = candidate.child(candidate.child_count() - 1)?;
        }
        (candidate.kind() == "$").then_some(())
    })())
    .is_some()
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
        ("nname", "methods") => vec![Insertion::MethodName],
        ("nname", "predicate" | "fact") => vec![Insertion::PredicateName],
        ("nname", "param") => vec![Insertion::ParamName],
        ("(", "predicate" | "fact") => vec![Insertion::PredicateName],
        ("(", "methods") => vec![Insertion::MethodName],
        ("variable", "term") => vec![Insertion::Variable],
        (";", "authorizer_element") => {
            trailing_origin_choice(node).unwrap_or_else(|| possible_children(parent.kind()))
        }
        (_, "ERROR") => recover_error_keyword_prefix(parent).unwrap_or_default(),
        (_, parent_kind) => possible_children(parent_kind),
    }
}

/// At `<rule|check|policy> |;`: the cursor sits right after a complete
/// `rule_body`, before the statement's terminating `;`. Nothing there is
/// directly insertable without a leading `,` except the optional trailing
/// origin clause, i.e. the `trusting` keyword itself — same signal as the
/// document-start `OriginClause` case, not `OriginElement` (which is for
/// *inside* an already-started clause, where `trusting` can't recur).
fn trailing_origin_choice(semicolon: ::tree_sitter::Node) -> Option<Vec<Insertion>> {
    let statement = semicolon.prev_sibling()?;
    if !matches!(statement.kind(), "rule" | "check" | "policy") {
        return None;
    }
    let rule_body = last_child(statement)?;
    if rule_body.kind() != "rule_body" {
        return None;
    }
    let has_origin_clause = last_child(rule_body).map(|n| n.kind()) == Some("origin_clause");
    (!has_origin_clause).then_some(vec![Insertion::OriginClause])
}

/// Recover when the whole statement collapses into a trailing `ERROR` directly
/// under `source_file` that nonetheless begins with a recognisable statement
/// keyword (e.g. `check if |`, cursor past the keyword and nothing else typed).
/// Unlike `children_in_context`'s `(_, "ERROR")` arm, the cursor here sits past
/// the ERROR node so the point-based descent lands on `source_file` itself,
/// leaving no parent to inspect — so we look at the root's last child directly.
fn trailing_error_keyword(root: ::tree_sitter::Node) -> Option<Vec<Insertion>> {
    recover_error_keyword_prefix(last_child(root)?)
}

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

fn possible_children(parent_kind: &str) -> Vec<Insertion> {
    use Insertion::*;
    match parent_kind {
        "rule" => vec![PredicateName], // rule head
        "rule_body" => vec![PredicateName, Term],
        "check" => vec![PredicateName, Term], // same as rule_body
        "policy" => vec![PredicateName, Term], // same as rule_body
        "expression" => vec![Term],
        "predicate" => vec![Term],
        "fact" => vec![FactTerm],
        "methods" => vec![Term], // MethodName only via children_in_context
        "array" => vec![FactTerm],
        "set" => vec![SetTerm],
        "parens" => vec![Term],
        "closure" => vec![Term],
        "method_argument" => vec![Term],
        "boolean" => vec![Term],
        "null" => vec![Term],
        "variable" => vec![Term],
        "term" => vec![Term],
        "fact_term" => vec![FactTerm],
        "set_term" => vec![SetTerm],
        "origin_clause" => vec![OriginElement],
        "origin_element" => vec![OriginElement],
        "authorizer_element" => vec![PredicateName, Policy, Check],
        _ => vec![],
    }
}

// the boolean allows inserting an extra space after the inserted keyword;
// the last field lists the `Insertion` contexts in which the keyword is valid
pub const KEYWORDS: &[(&str, &str, bool, &[Insertion])] = &[
    ("check if", "Check constraint", true, &[Insertion::Check]),
    (
        "check all",
        "Check all constraints",
        true,
        &[Insertion::Check],
    ),
    (
        "reject if",
        "Reject if condition",
        true,
        &[Insertion::Check],
    ),
    (
        "allow if",
        "Allow if condition (policy)",
        true,
        &[Insertion::Policy],
    ),
    (
        "deny if",
        "Deny if condition (policy)",
        true,
        &[Insertion::Policy],
    ),
    (
        "trusting",
        "Origin clause",
        true,
        &[Insertion::OriginClause],
    ),
    (
        "previous",
        "Trust previous block",
        false,
        &[Insertion::OriginElement],
    ),
    (
        "authority",
        "Trust authority block",
        false,
        &[Insertion::OriginElement],
    ),
    (
        "true",
        "Boolean true",
        false,
        &[Insertion::Term, Insertion::FactTerm, Insertion::SetTerm],
    ),
    (
        "false",
        "Boolean false",
        false,
        &[Insertion::Term, Insertion::FactTerm, Insertion::SetTerm],
    ),
    (
        "null",
        "Null value",
        false,
        &[Insertion::Term, Insertion::FactTerm, Insertion::SetTerm],
    ),
];

pub const METHODS: &[(&str, &str, bool)] = &[
    // Unary methods
    ("length", "Get length of string/array/set", false),
    ("type", "Get the type of the value", false),
    // Binary methods
    (
        "contains",
        "Check if string/array/set contains element",
        true,
    ),
    ("starts_with", "Check if string starts with prefix", true),
    ("ends_with", "Check if string ends with suffix", true),
    ("matches", "Check if string matches regex pattern", true),
    ("intersection", "Get intersection of two sets", true),
    ("union", "Get union of two sets", true),
    (
        "all",
        "Check if all elements satisfy a condition (closure)",
        true,
    ),
    (
        "any",
        "Check if any element satisfies a condition (closure)",
        true,
    ),
    ("get", "Get element from array/map", true),
    ("try_or", "Try operation or return fallback value", true),
];

pub fn predicate_name_node<'a>(node: &::tree_sitter::Node<'a>) -> Option<::tree_sitter::Node<'a>> {
    let name = match node.kind() {
        "fact" | "predicate" => node.child(0)?,
        _ => return None,
    };
    (name.kind() == "nname").then_some(name)
}

pub fn count_predicate_arguments(node: &::tree_sitter::Node) -> usize {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| matches!(child.kind(), "term" | "fact_term"))
        .count()
}

pub fn predicate_name_and_arity_query(
    name_node: &::tree_sitter::Node,
    parent: &::tree_sitter::Node,
    rope: &Rope,
) -> Option<(String, usize)> {
    matches!(parent.kind(), "fact" | "predicate").then(|| {
        (
            tree_sitter::node_text(name_node, rope),
            count_predicate_arguments(parent),
        )
    })
}

pub fn matching_predicate_name(
    node: &::tree_sitter::Node,
    rope: &Rope,
    name: &str,
    arity: usize,
) -> Option<tree_sitter::ByteRange> {
    let name_node = predicate_name_node(node)?;
    (tree_sitter::node_text(&name_node, rope) == name && count_predicate_arguments(node) == arity)
        .then(|| (name_node.start_byte(), name_node.end_byte()))
}

pub fn variable_nodes<'a>(scope: &::tree_sitter::Node<'a>) -> Vec<::tree_sitter::Node<'a>> {
    let mut nodes = Vec::new();
    tree_sitter::visit_node(scope, &mut |node| {
        if node.kind() == "variable" {
            nodes.push(*node);
        }
    });
    nodes
}

pub fn variable_occurrences(
    scope: &::tree_sitter::Node,
    rope: &Rope,
    var_name: &str,
) -> Vec<tree_sitter::ByteRange> {
    variable_nodes(scope)
        .into_iter()
        .filter(|node| tree_sitter::node_text(node, rope) == var_name)
        .map(|node| (node.start_byte(), node.end_byte()))
        .collect()
}

pub fn get_predicates(tree: &tree_sitter::Tree, rope: &Rope) -> Vec<(String, usize)> {
    let mut predicates = HashSet::new();
    tree_sitter::visit_node(&tree.root_node(), &mut |node| {
        if let Some(name_node) = predicate_name_node(node) {
            let name = tree_sitter::node_text(&name_node, rope);
            if !name.contains('\n') {
                predicates.insert((name, count_predicate_arguments(node)));
            }
        }
    });
    predicates.into_iter().collect()
}

pub fn get_variables_in_scope(
    context: ::tree_sitter::Node,
    rope: &Rope,
    byte_offset: usize,
) -> Vec<String> {
    variable_nodes(&context)
        .into_iter()
        // Exclude only the variable being typed (cursor within its range).
        .filter(|node| !(byte_offset >= node.start_byte() && byte_offset <= node.end_byte()))
        .map(|node| {
            let text = tree_sitter::node_text(&node, rope);
            text.strip_prefix('$').unwrap_or(&text).to_string()
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

/// Find the enclosing rule, check, or policy node for the given byte offset.
/// If no proper context is found, returns the nearest ERROR node as a fallback.
pub fn find_enclosing_context(
    root: ::tree_sitter::Node,
    byte_offset: usize,
) -> Option<::tree_sitter::Node> {
    let mut current = root.descendant_for_byte_range(byte_offset, byte_offset);

    // If we got the root and we're not at position 0, try one byte before
    // This handles cases where cursor is at the very end of content
    if let Some(node) = current {
        if node.kind() == "source_file" && byte_offset > 0 {
            current = root.descendant_for_byte_range(
                byte_offset.saturating_sub(1),
                byte_offset.saturating_sub(1),
            );
        }
    }

    let mut current = current?;
    let mut error_fallback = None;

    // Walk up the tree to find a rule, check, or policy node
    loop {
        match current.kind() {
            "rule" | "check" | "policy" => return Some(current),
            "ERROR"
                // Remember the ERROR node as a fallback
                if error_fallback.is_none() => {
                    error_fallback = Some(current);
                }
            _ => {}
        }

        // Before moving to parent, check if any siblings are rule/check/policy
        // This handles cases where cursor is at a sibling of the context (e.g., at ";")
        if let Some(parent) = current.parent() {
            let mut cursor = parent.walk();
            for sibling in parent.children(&mut cursor) {
                if matches!(sibling.kind(), "rule" | "check" | "policy")
                    && sibling.start_byte() <= byte_offset
                    && sibling.end_byte() >= byte_offset
                {
                    return Some(sibling);
                }
            }
            current = parent;
        } else {
            // Reached the root without finding a proper context
            // Use ERROR node as fallback if available
            return error_fallback;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree_sitter::{node_text, visit_node};
    use ::tree_sitter::Parser;
    use ropey::Rope;

    fn parse_biscuit(code: &str) -> (::tree_sitter::Tree, Rope) {
        let mut parser = Parser::new();
        parser
            .set_language(tree_sitter_biscuit::language())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let rope = Rope::from_str(code);
        (tree, rope)
    }

    fn find_first_node_of_kind<'a>(
        node: ::tree_sitter::Node<'a>,
        kind: &str,
    ) -> Option<::tree_sitter::Node<'a>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = find_first_node_of_kind(child, kind) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn test_find_enclosing_context_in_rule() {
        let (tree, _rope) = parse_biscuit("admin($u) <- user($u), role($u, \"admin\");");
        let context = find_enclosing_context(tree.root_node(), 20); // inside "user($u)"
        assert_eq!(context.unwrap().kind(), "rule");
    }

    #[test]
    fn test_find_enclosing_context_in_check() {
        let (tree, _rope) = parse_biscuit("check if user($u);");
        let context = find_enclosing_context(tree.root_node(), 12); // inside "user"
        assert_eq!(context.unwrap().kind(), "check");
    }

    #[test]
    fn test_find_enclosing_context_outside() {
        let (tree, _rope) = parse_biscuit("user(\"alice\");");
        // A fact is not a rule/check/policy, so there is no enclosing context.
        assert!(find_enclosing_context(tree.root_node(), 5).is_none());
    }

    #[test]
    fn test_count_predicate_arguments_fact_one() {
        let (tree, _rope) = parse_biscuit("user(\"alice\");");
        let fact = find_first_node_of_kind(tree.root_node(), "fact").unwrap();
        assert_eq!(count_predicate_arguments(&fact), 1);
    }

    #[test]
    fn test_count_predicate_arguments_fact_multiple() {
        let (tree, _rope) = parse_biscuit("role(\"alice\", \"admin\");");
        let fact = find_first_node_of_kind(tree.root_node(), "fact").unwrap();
        assert_eq!(count_predicate_arguments(&fact), 2);
    }

    #[test]
    fn test_count_predicate_arguments_one() {
        let (tree, _rope) = parse_biscuit("check if user($u);");
        let predicate = find_first_node_of_kind(tree.root_node(), "predicate").unwrap();
        assert_eq!(count_predicate_arguments(&predicate), 1);
    }

    #[test]
    fn test_count_predicate_arguments_multiple() {
        let (tree, rope) = parse_biscuit("admin($u) <- user($u), role($u, \"admin\");");

        let mut role_arities = Vec::new();
        visit_node(&tree.root_node(), &mut |node| {
            if node.kind() == "predicate"
                && predicate_name_node(node)
                    .map(|n| node_text(&n, &rope))
                    .as_deref()
                    == Some("role")
            {
                role_arities.push(count_predicate_arguments(node));
            }
        });

        assert_eq!(role_arities, vec![2]);
    }

    #[test]
    fn test_get_predicates_deduplicates() {
        // Same predicate name/arity appears as both fact and rule head.
        let (tree, rope) = parse_biscuit(
            r#"
            user("alice");
            user($u) <- admin($u);
            "#,
        );

        let predicates = get_predicates(&tree, &rope);
        assert_eq!(
            predicates
                .iter()
                .filter(|(name, arity)| name == "user" && *arity == 1)
                .count(),
            1
        );
        assert_eq!(
            predicates
                .iter()
                .filter(|(name, arity)| name == "admin" && *arity == 1)
                .count(),
            1
        );
    }

    #[test]
    fn test_get_predicates_multiline_error_node() {
        // Error recovery can produce an nname spanning lines; such names are skipped.
        let (tree, rope) = parse_biscuit("user\nanother_predicate();");
        for (name, _arity) in get_predicates(&tree, &rope) {
            assert!(!name.contains('\n'), "unexpected multiline name: {name:?}");
        }
    }
}
