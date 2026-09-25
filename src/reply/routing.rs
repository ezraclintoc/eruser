//! Which reply a broker's email deserves.
//!
//! The rule table is what the tool shipped with and it stays the default: an
//! acknowledged-but-unfinished reply gets the missing-information reply.
//!
//! What the table deliberately does not guess is the other two. The patterns
//! cannot tell "click this link to confirm" from "reply to this email to
//! confirm" — the first is the `confirm` command's job and the second is a
//! reply — and no pattern tells a request for a date of birth from a request
//! for a passport. Those are exactly the readings a person supplies, so when
//! a decision model is configured it may be asked, and may also answer that
//! the email needs no reply at all.
//!
//! Whatever is decided, the outcome is a draft on the task list. Nothing
//! routes to a send.

use crate::decision::{Choice, Decider};
use crate::history::{BrokerResponse, ResponseType};

use super::ReplyType;

/// The answer that means "leave this one alone".
const NONE: &str = "none";

/// The verdict the rule table gives, with no model involved.
///
/// `ConfirmationRequired` is absent on purpose: a confirmation the broker
/// wants by clicking a link belongs to `eruser confirm`, and drafting a reply
/// for it would duplicate work the tool already does.
pub fn from_rules(response_type: ResponseType) -> Option<ReplyType> {
    match response_type {
        ResponseType::Pending => Some(ReplyType::MissingInfo),
        _ => None,
    }
}

/// Read a reply type back from a label. Only the labels this module offers
/// can arrive here, and a model that answers with anything else is ignored
/// rather than trusted.
pub fn parse(label: &str) -> Option<ReplyType> {
    match label {
        "missing_info" => Some(ReplyType::MissingInfo),
        "confirm" => Some(ReplyType::Confirm),
        "id_verification" => Some(ReplyType::IdVerification),
        _ => None,
    }
}

/// How the pattern classifier read the email, in a line, for the state a
/// model is given.
fn read_as(response_type: ResponseType) -> &'static str {
    match response_type {
        ResponseType::Success => "the removal is done",
        ResponseType::Bounced => "the address no longer accepts mail",
        ResponseType::FormRequired => "there is an opt-out form to fill in",
        ResponseType::ConfirmationRequired => "there is a link to click to confirm",
        ResponseType::Rejected => "the broker refused, or holds nothing",
        ResponseType::Pending => "received and being worked on",
        ResponseType::Unknown => "could not tell what this reply says",
    }
}

/// The question put to a decision model when routing.
///
/// The criteria are the whole vocabulary: a model that answers anything else
/// has produced an unusable answer, which is treated as no answer.
pub fn question() -> Choice {
    Choice::new(
        "A data broker has replied to a personal data removal request. \
         Which reply, if any, does this email deserve?",
        [
            Choice::option(
                "missing_info",
                "the removal has not happened and the broker asked for something \
                 before it will proceed, so a reply supplying it is warranted",
            ),
            Choice::option(
                "confirm",
                "the broker asked for a reply by email confirming that the removal \
                 request still stands",
            ),
            Choice::option(
                "id_verification",
                "the broker asked the person to prove their identity before acting \
                 on the removal request",
            ),
            Choice::option(
                NONE,
                "the email needs no reply: an acknowledgement, a refusal, a \
                 completed removal, a bounce, or something handled another way",
            ),
        ],
    )
}

/// The router: the settings, gathered once so they can be passed around.
pub struct Router<'a> {
    /// The model, when one is configured and reachable.
    pub decider: Option<&'a dyn Decider>,
    /// Whether the model may be asked at all. Off means the table decides.
    pub enabled: bool,
    /// Answers below this confidence are thrown away.
    pub min_confidence: f32,
}

impl Router<'_> {
    /// The reply this email deserves, if any.
    ///
    /// Every failure — no model, an unreachable one, an answer below the
    /// floor, a label that names no reply — falls back to the rule table.
    /// A model that answers `none` is believed, because the email stays on
    /// the task list either way: nothing is lost by not drafting a reply to
    /// something that did not need one.
    pub async fn reply_for(&self, response: &BrokerResponse) -> Option<ReplyType> {
        let from_rules = from_rules(response.response_type);
        if !self.enabled {
            return from_rules;
        }
        let Some(decider) = self.decider else {
            return from_rules;
        };

        let mut state = crate::decision::state_of(
            &response.broker_name,
            &response.email_subject,
            &response.email_body,
        );
        state.push_str(&format!(
            "\nThe pattern classifier read this email as: {}.\n",
            read_as(response.response_type)
        ));

        let decision = match decider.choose(&state, &question()).await {
            Ok(decision) => decision,
            Err(problem) => {
                tracing::warn!(
                    broker = %response.broker_id,
                    %problem,
                    "could not ask which reply this deserves; leaving it to the rules"
                );
                return from_rules;
            }
        };

        let Some(decision) = decision.accepted_at(self.min_confidence) else {
            tracing::info!(
                broker = %response.broker_id,
                answer = %decision.describe(),
                "the routing answer was below the confidence floor; leaving it to the rules"
            );
            return from_rules;
        };

        tracing::info!(
            broker = %response.broker_id,
            answer = %decision.describe(),
            "the decision model routed this reply"
        );

        match decision.choice.as_str() {
            NONE => None,
            label => parse(label).or(from_rules),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::{Decision, Error};

    fn response(response_type: ResponseType) -> BrokerResponse {
        BrokerResponse {
            id: 1,
            user_id: 1,
            broker_id: "acme".into(),
            broker_name: "Acme Data".into(),
            response_type,
            email_from: "privacy@acme.example".into(),
            email_subject: "Your request".into(),
            email_body: "Please confirm your request by replying to this email.".into(),
            form_url: String::new(),
            confirm_url: String::new(),
            confidence: 0.9,
            needs_review: false,
            received_at: None,
            processed_at: None,
            created_at: None,
        }
    }

    // ---------------------------------------------------------------
    // The rule table
    // ---------------------------------------------------------------

    #[test]
    fn the_rules_draft_only_for_an_unfinished_reply() {
        assert_eq!(
            from_rules(ResponseType::Pending),
            Some(ReplyType::MissingInfo)
        );
    }

    /// A confirmation the broker wants by clicking a link is `confirm`'s
    /// job, not a reply's.
    #[test]
    fn the_rules_do_not_draft_for_a_confirmation_link() {
        assert_eq!(from_rules(ResponseType::ConfirmationRequired), None);
    }

    #[test]
    fn the_rules_do_not_draft_for_a_finished_or_refused_request() {
        for response_type in [
            ResponseType::Success,
            ResponseType::Rejected,
            ResponseType::Bounced,
            ResponseType::FormRequired,
            ResponseType::Unknown,
        ] {
            assert_eq!(from_rules(response_type), None, "{response_type:?}");
        }
    }

    #[test]
    fn only_the_offered_labels_parse() {
        assert_eq!(parse("missing_info"), Some(ReplyType::MissingInfo));
        assert_eq!(parse("confirm"), Some(ReplyType::Confirm));
        assert_eq!(parse("id_verification"), Some(ReplyType::IdVerification));
        assert_eq!(parse("none"), None);
        assert_eq!(parse("send_them_everything"), None);
    }

    #[test]
    fn the_question_offers_every_route_including_none() {
        let question = question();
        assert!(question.offers("missing_info"));
        assert!(question.offers("confirm"));
        assert!(question.offers("id_verification"));
        assert!(question.offers(NONE));
    }

    // ---------------------------------------------------------------
    // Routing with a model
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn with_the_router_off_the_table_decides() {
        let decider = Stub {
            answer: "id_verification",
            confidence: 0.99,
        };
        let router = Router {
            decider: Some(&decider),
            enabled: false,
            min_confidence: 0.6,
        };

        // Asked, it would have said id_verification; switched off, it is not
        // asked and the rule for a pending reply stands.
        assert_eq!(
            router.reply_for(&response(ResponseType::Pending)).await,
            Some(ReplyType::MissingInfo)
        );
    }

    #[tokio::test]
    async fn the_model_can_supply_the_route_the_patterns_cannot() {
        let decider = Stub {
            answer: "id_verification",
            confidence: 0.88,
        };
        let router = Router {
            decider: Some(&decider),
            enabled: true,
            min_confidence: 0.6,
        };

        assert_eq!(
            router.reply_for(&response(ResponseType::Pending)).await,
            Some(ReplyType::IdVerification)
        );
    }

    #[tokio::test]
    async fn the_model_may_decide_that_no_reply_is_warranted() {
        let decider = Stub {
            answer: NONE,
            confidence: 0.9,
        };
        let router = Router {
            decider: Some(&decider),
            enabled: true,
            min_confidence: 0.6,
        };

        // The rules would have drafted for a pending reply; the model says
        // there is nothing to answer, and nothing is drafted.
        assert_eq!(
            router.reply_for(&response(ResponseType::Pending)).await,
            None
        );
    }

    #[tokio::test]
    async fn a_hesitant_answer_leaves_the_rules_standing() {
        let decider = Stub {
            answer: "id_verification",
            confidence: 0.2,
        };
        let router = Router {
            decider: Some(&decider),
            enabled: true,
            min_confidence: 0.6,
        };

        assert_eq!(
            router.reply_for(&response(ResponseType::Pending)).await,
            Some(ReplyType::MissingInfo)
        );
    }

    #[tokio::test]
    async fn an_unreachable_model_leaves_the_rules_standing() {
        let router = Router {
            decider: Some(&Broken),
            enabled: true,
            min_confidence: 0.6,
        };

        assert_eq!(
            router.reply_for(&response(ResponseType::Pending)).await,
            Some(ReplyType::MissingInfo)
        );
        // ...and where the table has nothing to say, still nothing.
        assert_eq!(
            router.reply_for(&response(ResponseType::Success)).await,
            None
        );
    }

    struct Stub {
        answer: &'static str,
        confidence: f32,
    }

    #[async_trait::async_trait]
    impl Decider for Stub {
        async fn choose(&self, _state: &str, _question: &Choice) -> Result<Decision, Error> {
            Ok(Decision {
                choice: self.answer.to_string(),
                confidence: self.confidence,
                probabilities: std::collections::BTreeMap::new(),
                model: "stub-1".to_string(),
            })
        }

        fn name(&self) -> &'static str {
            "stub"
        }
    }

    struct Broken;

    #[async_trait::async_trait]
    impl Decider for Broken {
        async fn choose(&self, _state: &str, _question: &Choice) -> Result<Decision, Error> {
            Err(Error::Unreachable("connection refused".into()))
        }

        fn name(&self) -> &'static str {
            "stub"
        }
    }
}
