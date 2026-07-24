use crate::domain::ClassificationDecision;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheAction {
    Delete,
    Classify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitialAction {
    Confirm,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfirmationAction {
    Delete,
    Ignore,
    Requeue,
}

pub(crate) fn cache_action(is_confirmed_spam: bool) -> CacheAction {
    if is_confirmed_spam {
        CacheAction::Delete
    } else {
        CacheAction::Classify
    }
}

pub(crate) fn initial_action(decision: &ClassificationDecision) -> InitialAction {
    if decision.spam {
        InitialAction::Confirm
    } else {
        InitialAction::Ignore
    }
}

pub(crate) fn confirmation_action(
    decision: Option<&ClassificationDecision>,
    shutting_down: bool,
) -> ConfirmationAction {
    match decision {
        Some(decision) if decision.spam => ConfirmationAction::Delete,
        Some(_) => ConfirmationAction::Ignore,
        None if shutting_down => ConfirmationAction::Ignore,
        None => ConfirmationAction::Requeue,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cache_action, confirmation_action, initial_action, CacheAction, ConfirmationAction,
        InitialAction,
    };
    use crate::domain::ClassificationDecision;

    fn decision(spam: bool) -> ClassificationDecision {
        ClassificationDecision { spam, reason: None }
    }

    #[test]
    fn confirmed_cache_hit_deletes_without_classification() {
        assert_eq!(cache_action(true), CacheAction::Delete);
    }

    #[test]
    fn cache_miss_requires_classification() {
        assert_eq!(cache_action(false), CacheAction::Classify);
    }

    #[test]
    fn initial_normal_decision_is_ignored() {
        assert_eq!(initial_action(&decision(false)), InitialAction::Ignore);
    }

    #[test]
    fn initial_spam_decision_requires_confirmation() {
        assert_eq!(initial_action(&decision(true)), InitialAction::Confirm);
    }

    #[test]
    fn confirmed_spam_is_deleted() {
        assert_eq!(
            confirmation_action(Some(&decision(true)), false),
            ConfirmationAction::Delete
        );
    }

    #[test]
    fn rejected_confirmation_is_not_deleted() {
        assert_eq!(
            confirmation_action(Some(&decision(false)), false),
            ConfirmationAction::Ignore
        );
    }

    #[test]
    fn missing_confirmation_is_requeued_unless_shutting_down() {
        assert_eq!(
            confirmation_action(None, false),
            ConfirmationAction::Requeue
        );
        assert_eq!(confirmation_action(None, true), ConfirmationAction::Ignore);
    }
}
