/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */

use ropey::Rope;

use crate::ast::{
    find_enclosing_context, matching_predicate_name, predicate_name_and_arity_query,
    variable_occurrences,
};
use crate::tree_sitter::{find_node_at_cursor, node_text, visit_node, ByteRange, Tree};

pub fn find_definition(tree: &Tree, rope: &Rope, byte_offset: usize) -> Option<ByteRange> {
    let node = find_node_at_cursor(tree.root_node(), byte_offset);

    match node.kind() {
        "variable" => {
            // The first occurrence in scope is the binding site; occurrences are
            // returned in document order, so the earliest is first.
            let context = find_enclosing_context(tree.root_node(), byte_offset)?;
            variable_occurrences(&context, rope, &node_text(&node, rope))
                .into_iter()
                .next()
        }
        "nname" => {
            let parent = node.parent()?;
            let (name, arity) = predicate_name_and_arity_query(&node, &parent, rope)?;
            find_predicate_definition(tree.root_node(), rope, &name, arity)
        }
        _ => None,
    }
}

fn find_predicate_definition(
    root: ::tree_sitter::Node,
    rope: &Rope,
    name: &str,
    arity: usize,
) -> Option<ByteRange> {
    let mut definition: Option<ByteRange> = None;
    visit_node(&root, &mut |node| match node.kind() {
        "fact" => {
            if let Some(range) = matching_predicate_name(node, rope, name, arity) {
                definition = Some(range);
            }
        }
        "rule" => {
            if let Some(head) = node.child_by_field_name("head") {
                if let Some(range) = matching_predicate_name(&head, rope, name, arity) {
                    definition = Some(range);
                }
            }
        }
        _ => {}
    });
    definition
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree_sitter::DocumentData;

    /// Resolve the definition for the cursor marked by `|` in `marked`, and
    /// return the resolved definition's source text.
    fn definition_text(marked: &str) -> Option<String> {
        let offset = marked.find('|').expect("test input needs a | cursor");
        let code = marked.replace('|', "");
        let doc = DocumentData::from_text(&code);
        let tree = doc.tree.as_ref().unwrap();
        let (start, end) = find_definition(tree, &doc.rope, offset)?;
        Some(doc.rope.byte_slice(start..end).to_string())
    }

    /// Byte offset of the definition for the cursor marked by `|`.
    fn definition_offset(marked: &str) -> Option<usize> {
        let offset = marked.find('|').unwrap();
        let code = marked.replace('|', "");
        let doc = DocumentData::from_text(&code);
        let tree = doc.tree.as_ref().unwrap();
        find_definition(tree, &doc.rope, offset).map(|(start, _)| start)
    }

    #[test]
    fn variable_resolves_to_first_occurrence_in_scope() {
        // Cursor inside the second `$u`; definition is the first `$u` (head).
        let code = "right($u) <- user($|u);";
        assert_eq!(definition_offset(code), Some(code.find("$u").unwrap()));
    }

    #[test]
    fn variable_scope_is_the_enclosing_statement() {
        // Two statements sharing a variable name: resolution stays local.
        let src = "a($x) <- b($x);\nc($x) <- d($|x);";
        let offset = definition_offset(src).unwrap();
        // The resolved `$x` must be in the second statement, not the first.
        assert!(offset > src.find(';').unwrap());
    }

    #[test]
    fn predicate_resolves_to_fact_definition() {
        assert_eq!(
            definition_text("user(\"alice\");\ncheck if us|er(\"alice\");"),
            Some("user".to_string())
        );
    }

    #[test]
    fn predicate_resolves_to_rule_head() {
        let offset =
            definition_offset("admin($u) <- user($u);\ncheck if adm|in(\"alice\");").unwrap();
        assert_eq!(offset, 0); // the rule head `admin` at the start of the file
    }

    #[test]
    fn arity_disambiguates_definitions() {
        // Two `user` facts of different arity; the 2-arg reference resolves to
        // the 2-arg fact, not the 1-arg one that appears first.
        let src = "user(\"alice\");\nuser(\"alice\", 1);\ncheck if us|er($a, $b);";
        let offset = definition_offset(src).unwrap();
        assert_eq!(offset, src.find("user(\"alice\", 1)").unwrap());
    }

    #[test]
    fn unresolvable_predicate_returns_none() {
        assert_eq!(definition_text("check if mis|sing(\"x\");"), None);
    }
}
