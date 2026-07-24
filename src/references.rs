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

pub fn find_references(tree: &Tree, rope: &Rope, byte_offset: usize) -> Vec<ByteRange> {
    let node = find_node_at_cursor(tree.root_node(), byte_offset);

    match node.kind() {
        "variable" => match find_enclosing_context(tree.root_node(), byte_offset) {
            Some(context) => variable_occurrences(&context, rope, &node_text(&node, rope)),
            None => Vec::new(),
        },
        "nname" => match node
            .parent()
            .and_then(|p| predicate_name_and_arity_query(&node, &p, rope))
        {
            Some((name, arity)) => find_predicate_references(tree.root_node(), rope, &name, arity),
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn find_predicate_references(
    root: ::tree_sitter::Node,
    rope: &Rope,
    name: &str,
    arity: usize,
) -> Vec<ByteRange> {
    let mut references = Vec::new();
    visit_node(&root, &mut |node| {
        if matches!(node.kind(), "fact" | "predicate") {
            if let Some(range) = matching_predicate_name(node, rope, name, arity) {
                references.push(range);
            }
        }
    });
    references
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree_sitter::DocumentData;

    /// Return the sorted set of definition offsets found for the cursor marked
    /// by `|` in `marked`.
    fn reference_offsets(marked: &str) -> Vec<usize> {
        let offset = marked.find('|').expect("test input needs a | cursor");
        let code = marked.replace('|', "");
        let doc = DocumentData::from_text(&code);
        let tree = doc.tree.as_ref().unwrap();
        let mut offsets: Vec<usize> = find_references(tree, &doc.rope, offset)
            .into_iter()
            .map(|(start, _)| start)
            .collect();
        offsets.sort_unstable();
        offsets
    }

    #[test]
    fn variable_references_within_scope() {
        // `$u` appears three times in the rule; all three are references.
        let code = "right($u) <- user($|u), owner($u);";
        let expected: Vec<usize> = code
            .replace('|', "")
            .match_indices("$u")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(reference_offsets(code), expected);
    }

    #[test]
    fn variable_references_stay_in_scope() {
        // The same name in another statement must not leak in.
        let src = "a($x) <- b($x);\nc($x) <- d($|x);";
        let offsets = reference_offsets(src);
        // Only the two `$x` in the second statement.
        assert_eq!(offsets.len(), 2);
        assert!(offsets.iter().all(|&o| o > src.find(';').unwrap()));
    }

    #[test]
    fn predicate_references_span_document() {
        // The fact declaration and the predicate usage both count.
        let src = "user(\"alice\");\ncheck if us|er(\"alice\");";
        let code = src.replace('|', "");
        assert_eq!(
            reference_offsets(src),
            vec![code.find("user").unwrap(), code.rfind("user").unwrap()]
        );
    }

    #[test]
    fn predicate_references_respect_arity() {
        // A same-named fact of different arity is excluded.
        let src = "user($a);\nuser($a, $b);\ncheck if us|er($x, $y);";
        let offsets = reference_offsets(src);
        assert_eq!(offsets.len(), 2);
        assert!(offsets
            .iter()
            .all(|&o| o >= src.find("user($a, $b)").unwrap()));
    }

    #[test]
    fn non_predicate_has_no_references() {
        assert_eq!(
            reference_offsets("check if user(\"al|ice\");"),
            Vec::<usize>::new()
        );
    }
}
