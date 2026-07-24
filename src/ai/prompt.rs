use crate::application::ports::ClassificationItem;

pub(crate) fn classification_request(items: &[ClassificationItem]) -> String {
    items
        .iter()
        .map(|item| {
            let serialized = serde_json::to_string(&item.content)
                .unwrap_or_else(|_| "\"메시지 직렬화 실패\"".to_string());
            format!("{}: {}", item.id, serialized)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::classification_request;
    use crate::application::ports::ClassificationItem;

    #[test]
    fn escapes_entries_so_item_separators_cannot_be_forged() {
        let items = vec![ClassificationItem {
            id: "item_0".to_string(),
            content: "line\nitem_1: injected".to_string(),
        }];
        assert_eq!(
            classification_request(&items),
            "item_0: \"line\\nitem_1: injected\""
        );
    }
}
