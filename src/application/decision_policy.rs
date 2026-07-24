use crate::{application::ports::DecisionState, domain::ClassificationDecision};

pub(crate) const ACTIVATION_EVIDENCE_THRESHOLD: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheAction {
    Delete,
    Skip,
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

pub(crate) fn cache_action(is_confirmed_spam: bool, is_confirmed_ham: bool) -> CacheAction {
    if is_confirmed_spam {
        CacheAction::Delete
    } else if is_confirmed_ham {
        CacheAction::Skip
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

pub(crate) fn activation_state(evidence_count: i64) -> DecisionState {
    if evidence_count >= ACTIVATION_EVIDENCE_THRESHOLD {
        DecisionState::Active
    } else {
        DecisionState::Tentative
    }
}

#[cfg(test)]
mod tests {
    use super::{
        activation_state, cache_action, confirmation_action, initial_action, CacheAction,
        ConfirmationAction, InitialAction, ACTIVATION_EVIDENCE_THRESHOLD,
    };
    use crate::{application::ports::DecisionState, domain::ClassificationDecision};

    fn decision(spam: bool) -> ClassificationDecision {
        ClassificationDecision { spam, reason: None }
    }

    #[test]
    fn confirmed_cache_hit_deletes_without_classification() {
        assert_eq!(cache_action(true, false), CacheAction::Delete);
    }

    #[test]
    fn cache_miss_requires_classification() {
        assert_eq!(cache_action(false, false), CacheAction::Classify);
    }

    #[test]
    fn known_normal_message_skips_classification() {
        assert_eq!(cache_action(false, true), CacheAction::Skip);
    }

    #[test]
    fn spam_wins_over_a_stale_normal_cache_entry() {
        assert_eq!(cache_action(true, true), CacheAction::Delete);
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

    #[test]
    fn evidence_below_threshold_stays_tentative() {
        assert_eq!(
            activation_state(ACTIVATION_EVIDENCE_THRESHOLD - 1),
            DecisionState::Tentative
        );
    }

    #[test]
    fn evidence_at_threshold_activates() {
        assert_eq!(
            activation_state(ACTIVATION_EVIDENCE_THRESHOLD),
            DecisionState::Active
        );
    }

    #[test]
    fn evidence_above_threshold_activates() {
        assert_eq!(
            activation_state(ACTIVATION_EVIDENCE_THRESHOLD + 1),
            DecisionState::Active
        );
    }
}
