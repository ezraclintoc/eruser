//! The pre-written replies, and choosing between them.
//!
//! Generating a paragraph and then checking it for promises is a lot of
//! machinery to arrive at "please go ahead with the deletion". The wording
//! brokers need is short, repetitive, and known in advance, so it ships
//! written: a handful of candidates per reply type, checked into the
//! repository beside the request letters, and the only open question is
//! which one fits.
//!
//! That question is a `choice`, which is what a System One model answers
//! best — and because the answer is the *name* of a candidate rather than a
//! paragraph, nothing the model produces can end up in the email. The worst
//! it can do is name the wrong candidate, and a person reads every draft
//! before it goes anywhere.
//!
//! No decider is needed to use the library: with none configured the
//! first-fitting candidate is taken, so the wording is still consistent from
//! one month to the next.

use crate::config::Profile;
use crate::decision::{Choice, Decider};
use crate::template::Engine;

use super::ReplyType;

/// One pre-written reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// Stable name, recorded on the draft and used as the answer label.
    pub name: &'static str,
    /// One line telling a decision model when this one is the right one.
    pub when: &'static str,
    /// The body, rendered against the person's own details.
    pub body: &'static str,
}

/// Every candidate for a reply type, best first.
///
/// The order is the fallback: with nothing choosing, the first one is used,
/// so it should be the one that is safe in the most cases.
pub fn candidates(reply_type: ReplyType) -> &'static [Candidate] {
    match reply_type {
        ReplyType::MissingInfo => MISSING_INFO,
        ReplyType::Confirm => CONFIRM,
        ReplyType::IdVerification => ID_VERIFICATION,
    }
}

/// One line describing a reply type, for the choice's instructions.
fn describes(reply_type: ReplyType) -> &'static str {
    match reply_type {
        ReplyType::MissingInfo => {
            "The broker has not finished the removal and asked for something. \
             Which of these replies fits what they asked for?"
        }
        ReplyType::Confirm => {
            "The broker asked for a reply by email confirming that the removal \
             request still stands. Which of these replies fits?"
        }
        ReplyType::IdVerification => {
            "The broker has asked for identity verification before acting on a \
             removal request. Which of these replies fits what they asked for?"
        }
    }
}

static MISSING_INFO: &[Candidate] = &[
    Candidate {
        name: "provide_details",
        when: "the broker named the fields it wants, so give them from the profile",
        body: include_str!("../../templates/replies/missing_info/provide_details.txt"),
    },
    Candidate {
        name: "restate_request",
        when: "the broker wants details that the original request already contained",
        body: include_str!("../../templates/replies/missing_info/restate_request.txt"),
    },
    Candidate {
        name: "ask_what_is_missing",
        when: "the broker said something was missing but did not say what",
        body: include_str!("../../templates/replies/missing_info/ask_what_is_missing.txt"),
    },
];

static CONFIRM: &[Candidate] = &[
    Candidate {
        name: "confirm_stands",
        when: "the broker only asked to be told that the request still stands",
        body: include_str!("../../templates/replies/confirm/confirm_stands.txt"),
    },
    Candidate {
        name: "confirm_repeat_details",
        when: "the broker asked to confirm and to restate the identifying details",
        body: include_str!("../../templates/replies/confirm/confirm_repeat_details.txt"),
    },
];

static ID_VERIFICATION: &[Candidate] = &[
    Candidate {
        name: "ask_what_verification",
        when: "the broker asked for verification without saying what it involves",
        body: include_str!("../../templates/replies/id_verification/ask_what_verification.txt"),
    },
    Candidate {
        name: "ask_without_documents",
        when: "the broker asked for a copy of an identity document",
        body: include_str!("../../templates/replies/id_verification/ask_without_documents.txt"),
    },
];

/// The question put to a decision model: one option per candidate.
pub fn question(reply_type: ReplyType) -> Choice {
    Choice::new(
        describes(reply_type),
        candidates(reply_type)
            .iter()
            .map(|candidate| Choice::option(candidate.name, candidate.when)),
    )
}

/// How a candidate was chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct Chosen {
    pub candidate: &'static Candidate,
    /// `rules` when the first candidate was taken, otherwise the name of
    /// whatever chose.
    pub by: &'static str,
    /// The model's confidence, when a model chose.
    pub confidence: Option<f32>,
}

impl Chosen {
    /// What to record as the producer of the draft, e.g.
    /// `canned:provide_details`.
    pub fn producer(&self) -> String {
        format!("canned:{}", self.candidate.name)
    }
}

/// Pick the candidate to use, asking the model when there is one.
///
/// Every way of failing — no decider, an unreachable one, an answer below
/// the confidence floor, a label that names no candidate — lands on the
/// first candidate for the type. A library with a model in front of it
/// produces better-fitting replies; a library without one still produces
/// replies.
pub async fn choose(
    decider: Option<&dyn Decider>,
    min_confidence: f32,
    reply_type: ReplyType,
    state: &str,
) -> Option<Chosen> {
    let all = candidates(reply_type);
    let fallback = all.first()?;

    let Some(decider) = decider else {
        return Some(Chosen {
            candidate: fallback,
            by: "rules",
            confidence: None,
        });
    };

    match decider.choose(state, &question(reply_type)).await {
        Ok(decision) => {
            // A low-confidence answer is no answer: the shipped wording is a
            // known quantity, and a coin-flip between candidates is not an
            // improvement on it.
            let Some(decision) = decision.accepted_at(min_confidence) else {
                tracing::info!(
                    chosen = %decision.describe(),
                    "the wording decision was below the confidence floor; using the first candidate"
                );
                return Some(Chosen {
                    candidate: fallback,
                    by: "rules",
                    confidence: None,
                });
            };

            match all
                .iter()
                .find(|candidate| candidate.name == decision.choice)
            {
                Some(candidate) => Some(Chosen {
                    candidate,
                    by: decider.name(),
                    confidence: Some(decision.confidence),
                }),
                None => Some(Chosen {
                    candidate: fallback,
                    by: "rules",
                    confidence: None,
                }),
            }
        }
        Err(problem) => {
            tracing::warn!(%problem, "could not have the wording chosen; using the first candidate");
            Some(Chosen {
                candidate: fallback,
                by: "rules",
                confidence: None,
            })
        }
    }
}

/// Render a candidate against the person's own details.
///
/// The same data a request letter gets, so a reply can quote a name and an
/// address exactly as the request did, and the same strict-undefined rule:
/// a typo in a candidate fails here rather than being sent with a hole in it.
pub fn render(
    engine: &Engine,
    candidate: &Candidate,
    profile: &Profile,
    broker_name: &str,
) -> Result<String, crate::template::Error> {
    engine.render_reply(candidate.name, candidate.body, profile, broker_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reply::{RawDraft, ReplyContext, validate};

    const TYPES: [ReplyType; 3] = [
        ReplyType::MissingInfo,
        ReplyType::Confirm,
        ReplyType::IdVerification,
    ];

    fn profile() -> Profile {
        Profile {
            first_name: "Jane".into(),
            last_name: "Doe".into(),
            email: "jane@example.com".into(),
            address: "1 Main Street".into(),
            city: "Springfield".into(),
            state: "IL".into(),
            zip_code: "62701".into(),
            country: "USA".into(),
            phone: "+1-555-0100".into(),
            date_of_birth: "1990-01-15".into(),
        }
    }

    #[test]
    fn every_reply_type_has_candidates_and_the_first_is_the_fallback() {
        for reply_type in TYPES {
            let all = candidates(reply_type);
            assert!(!all.is_empty(), "{reply_type:?} has no candidates");
        }
    }

    #[test]
    fn candidate_names_are_unique_within_a_reply_type() {
        for reply_type in TYPES {
            let all = candidates(reply_type);
            let mut names: Vec<&str> = all.iter().map(|c| c.name).collect();
            names.sort_unstable();
            let count = names.len();
            names.dedup();
            assert_eq!(names.len(), count, "a duplicate name in {reply_type:?}");
        }
    }

    #[test]
    fn every_candidate_is_described_for_the_model() {
        for reply_type in TYPES {
            for candidate in candidates(reply_type) {
                assert!(
                    !candidate.when.trim().is_empty(),
                    "{} has no criterion",
                    candidate.name
                );
            }
        }
    }

    /// The question offers exactly the candidates, so an answer can only
    /// ever name one of them.
    #[test]
    fn the_question_offers_every_candidate() {
        for reply_type in TYPES {
            let question = question(reply_type);
            for candidate in candidates(reply_type) {
                assert!(
                    question.offers(candidate.name),
                    "{} is missing from the {reply_type:?} question",
                    candidate.name
                );
            }
            assert_eq!(question.options.len(), candidates(reply_type).len());
        }
    }

    #[test]
    fn every_candidate_renders_and_quotes_the_person_details() {
        let engine = Engine::new().expect("the engine");
        for reply_type in TYPES {
            for candidate in candidates(reply_type) {
                let body = render(&engine, candidate, &profile(), "Acme Data")
                    .unwrap_or_else(|problem| panic!("{}: {problem}", candidate.name));

                assert!(
                    body.contains("Acme Data"),
                    "{} does not address the broker",
                    candidate.name
                );
                assert!(
                    body.contains("Jane Doe"),
                    "{} omits the name",
                    candidate.name
                );
                // Nothing is left unrendered, and no optional line is left
                // with a hole where a blank field was.
                assert!(
                    !body.contains("{{"),
                    "{} has an unrendered tag",
                    candidate.name
                );
            }
        }
    }

    /// The shipped wording is eruser's own, so it has to clear eruser's own
    /// rules. A candidate that promises documents or agrees to something is
    /// a bug in the candidate, and this is what catches it before a person
    /// ever sees the draft.
    #[test]
    fn every_candidate_passes_the_validator() {
        let engine = Engine::new().expect("the engine");
        for reply_type in TYPES {
            for candidate in candidates(reply_type) {
                let body = render(&engine, candidate, &profile(), "Acme Data")
                    .expect("a rendered candidate");
                let context = ReplyContext {
                    reply_type,
                    broker_name: "Acme Data".into(),
                    original_subject: "Your removal request".into(),
                    original_body: "Please confirm.".into(),
                    sent_subject: String::new(),
                    profile: profile(),
                };

                validate(&RawDraft { body }, &context).unwrap_or_else(|problem| {
                    panic!(
                        "{} is refused by eruser's own rules: {problem}",
                        candidate.name
                    )
                });
            }
        }
    }

    /// A candidate is only useful if it survives the profile being sparse —
    /// the setup flow only insists on a name and an email.
    #[test]
    fn every_candidate_renders_for_a_minimal_profile() {
        let engine = Engine::new().expect("the engine");
        let minimal = Profile {
            first_name: "Jane".into(),
            email: "jane@example.com".into(),
            ..Default::default()
        };

        for reply_type in TYPES {
            for candidate in candidates(reply_type) {
                let body = render(&engine, candidate, &minimal, "Acme Data")
                    .unwrap_or_else(|problem| panic!("{}: {problem}", candidate.name));
                assert!(
                    !body.contains("Phone:") && !body.contains("Address:"),
                    "{} lists a field the profile does not have",
                    candidate.name
                );
            }
        }
    }

    #[test]
    fn the_producer_names_the_candidate() {
        let chosen = Chosen {
            candidate: &MISSING_INFO[0],
            by: "rules",
            confidence: None,
        };
        assert_eq!(chosen.producer(), "canned:provide_details");
    }

    // ---------------------------------------------------------------
    // Choosing, without a model on the other end
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn with_no_decider_the_first_candidate_is_taken() {
        let chosen = choose(None, 0.6, ReplyType::MissingInfo, "state")
            .await
            .expect("a candidate");

        assert_eq!(chosen.candidate.name, "provide_details");
        assert_eq!(chosen.by, "rules");
        assert_eq!(chosen.confidence, None);
    }

    /// A decider that answers with a label it was never offered, or with
    /// less confidence than the floor allows, leaves the shipped wording in
    /// place rather than picking something at random.
    #[tokio::test]
    async fn a_low_confidence_answer_falls_back_to_the_shipped_wording() {
        let decider = StubDecider {
            answer: "ask_what_is_missing",
            confidence: 0.2,
        };

        let chosen = choose(Some(&decider), 0.6, ReplyType::MissingInfo, "state")
            .await
            .expect("a candidate");
        assert_eq!(chosen.candidate.name, "provide_details");
        assert_eq!(chosen.by, "rules");
    }

    #[tokio::test]
    async fn a_confident_answer_picks_the_candidate_it_names() {
        let decider = StubDecider {
            answer: "ask_what_is_missing",
            confidence: 0.91,
        };

        let chosen = choose(Some(&decider), 0.6, ReplyType::MissingInfo, "state")
            .await
            .expect("a candidate");

        assert_eq!(chosen.candidate.name, "ask_what_is_missing");
        assert_eq!(chosen.by, "stub");
        assert_eq!(chosen.confidence, Some(0.91));
    }

    #[tokio::test]
    async fn an_unreachable_decider_falls_back_to_the_shipped_wording() {
        let decider = BrokenDecider;

        let chosen = choose(Some(&decider), 0.6, ReplyType::Confirm, "state")
            .await
            .expect("a candidate");
        assert_eq!(chosen.candidate.name, "confirm_stands");
        assert_eq!(chosen.by, "rules");
    }

    struct StubDecider {
        answer: &'static str,
        confidence: f32,
    }

    #[async_trait::async_trait]
    impl Decider for StubDecider {
        async fn choose(
            &self,
            _state: &str,
            _question: &Choice,
        ) -> Result<crate::decision::Decision, crate::decision::Error> {
            Ok(crate::decision::Decision {
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

    struct BrokenDecider;

    #[async_trait::async_trait]
    impl Decider for BrokenDecider {
        async fn choose(
            &self,
            _state: &str,
            _question: &Choice,
        ) -> Result<crate::decision::Decision, crate::decision::Error> {
            Err(crate::decision::Error::Unreachable(
                "connection refused".into(),
            ))
        }

        fn name(&self) -> &'static str {
            "stub"
        }
    }
}
