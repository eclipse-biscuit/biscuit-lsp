/*
 * SPDX-FileCopyrightText: 2023 Clément Delafargue <clement@delafargue.name>
 *
 * SPDX-License-Identifier: Apache-2.0
 */
use ropey::Rope;

use crate::references::find_references;
use crate::tree_sitter::{find_node_at_cursor, ByteRange, Tree};

/// A single rename edit: replace `range` with `new_text`.
pub type RenameEdit = (ByteRange, String);

pub fn rename_edits(
    tree: &Tree,
    rope: &Rope,
    byte_offset: usize,
    new_name: &str,
) -> Option<Vec<RenameEdit>> {
    let node = find_node_at_cursor(tree.root_node(), byte_offset);
    let is_variable = match node.kind() {
        "variable" => true,
        "nname" => false,
        _ => return None,
    };

    let occurrences = find_references(tree, rope, byte_offset);
    if occurrences.is_empty() {
        return None;
    }

    // Variable occurrences span the leading `$`, so the replacement must keep
    // it regardless of whether the user typed it.
    let new_text = if is_variable {
        format!("${}", new_name.strip_prefix('$').unwrap_or(new_name))
    } else {
        new_name.to_string()
    };

    Some(
        occurrences
            .into_iter()
            .map(|range| (range, new_text.clone()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree_sitter::DocumentData;

    /// Apply the rename triggered at the `|` cursor in `marked` and return the
    /// resulting document text, or `None` if nothing is renameable.
    fn apply_rename(marked: &str, new_name: &str) -> Option<String> {
        let offset = marked.find('|').expect("test input needs a | cursor");
        let code = marked.replace('|', "");
        let doc = DocumentData::from_text(&code);
        let tree = doc.tree.as_ref().unwrap();
        let mut edits = rename_edits(tree, &doc.rope, offset, new_name)?;

        // Apply from the end so earlier offsets stay valid.
        edits.sort_by_key(|((start, _), _)| *start);
        let mut result = code;
        for ((start, end), new_text) in edits.into_iter().rev() {
            result.replace_range(start..end, &new_text);
        }
        Some(result)
    }

    #[test]
    fn renames_variable_in_scope() {
        assert_eq!(
            apply_rename("right($u) <- user($|u), owner($u);", "person"),
            Some("right($person) <- user($person), owner($person);".to_string())
        );
    }

    #[test]
    fn variable_rename_normalizes_dollar_prefix() {
        // A leading `$` typed by the user must not be doubled.
        assert_eq!(
            apply_rename("check if user($|u);", "$who"),
            Some("check if user($who);".to_string())
        );
    }

    #[test]
    fn variable_rename_stays_in_scope() {
        // The `$x` in the first statement is untouched.
        assert_eq!(
            apply_rename("a($x) <- b($x);\nc($x) <- d($|x);", "y"),
            Some("a($x) <- b($x);\nc($y) <- d($y);".to_string())
        );
    }

    #[test]
    fn renames_predicate_by_name_and_arity() {
        // Only the 1-arg `user` occurrences are renamed, not the 2-arg one.
        assert_eq!(
            apply_rename("user($a);\nuser($a, $b);\ncheck if us|er($x);", "account"),
            Some("account($a);\nuser($a, $b);\ncheck if account($x);".to_string())
        );
    }

    #[test]
    fn non_predicate_is_not_renameable() {
        assert_eq!(apply_rename("check if user(\"al|ice\");", "x"), None);
    }
}
