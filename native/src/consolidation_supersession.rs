//! Source-independent same-id replacement decisions. Evidence and predicates are
//! deliberately lazy: an invalid replacement shape never asks for either, and a
//! dated/cut/fired page decides before the standing predicate is evaluated.
use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decision {
    pub allowed: bool,
    pub reason: String,
}
pub(crate) struct PageFacts {
    pub linked: bool,
    pub fired: bool,
    pub cut: Option<String>,
    pub reading: Option<String>,
}
pub(crate) struct JudgmentEvidence {
    pub stamp: String,
    pub born: Option<String>,
    pub page: Option<PageFacts>,
    pub predicate: String,
}
pub(crate) fn judgment(
    replacement_shaped: bool,
    deps: &str,
    by_hand: bool,
    evidence: impl FnOnce() -> Result<JudgmentEvidence>,
    standing_predicate: impl FnOnce() -> Result<Option<bool>>,
) -> Result<Decision> {
    if !replacement_shaped {
        return Ok(Decision {
            allowed: false,
            reason: format!(
                "what replaces a judgment must rest on something, and this carries no {deps}"
            ),
        });
    }
    let evidence = evidence()?;
    if let Some(page) = evidence.page {
        if let Some(born) = evidence.born.filter(|born| *born >= evidence.stamp) {
            return Ok(Decision {
                allowed: false,
                reason: format!(
                    "it was decided on {born} - a second decision on the same day is a contradiction, not a change"
                ),
            });
        }
        if !page.linked {
            let cut = page.cut.ok_or_else(|| Error("invalid_page_facts".into()))?;
            return Ok(Decision {
                allowed: false,
                reason: format!(
                    "the brief no longer carries what it decided - {cut} - restore the tab, then re-decide"
                ),
            });
        }
        if page.fired {
            return Ok(Decision {
                allowed: true,
                reason: format!(
                    "its sign holds ({}) with its tab intact{}",
                    evidence.predicate,
                    page.reading.map(|v| format!(" - {v}")).unwrap_or_default()
                ),
            });
        }
    }
    let fired = standing_predicate()? == Some(true);
    Ok(Decision {
        allowed: fired || by_hand,
        reason: if fired {
            format!("its wrong_if holds ({})", evidence.predicate)
        } else if by_hand {
            "the standing judgment holds, and a person takes this over it by name".into()
        } else {
            "the standing judgment holds, and its wrong_if has not fired".into()
        },
    })
}
pub(crate) fn reading(stamp: &str, prior_day: Option<&str>) -> Decision {
    Decision {
        allowed: prior_day.is_none_or(|day| stamp > day),
        reason: match prior_day {
            None => "nothing dates the reading the base holds".into(),
            Some(day) if stamp > day => {
                format!("a reading from {stamp} that is newer than the base's")
            }
            Some(day) if stamp == day => "a reading of the same day".into(),
            Some(_) => format!("a reading from {stamp} that is older than the base's"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shape_and_page_short_circuit_the_predicate() {
        let failure = || -> Result<Option<bool>> { panic!("predicate must stay lazy") };
        let invalid = judgment(
            false,
            "rests_on",
            false,
            || panic!("evidence must stay lazy"),
            failure,
        )
        .unwrap();
        assert!(!invalid.allowed);
        for (born, linked, fired, allowed) in [
            (Some("2026-09-20"), true, true, false),
            (None, false, true, false),
            (None, true, true, true),
        ] {
            let decision = judgment(
                true,
                "rests_on",
                true,
                || {
                    Ok(JudgmentEvidence {
                        stamp: "2026-09-20".into(),
                        born: born.map(str::to_owned),
                        page: Some(PageFacts {
                            linked,
                            fired,
                            cut: Some("missing".into()),
                            reading: None,
                        }),
                        predicate: "p.value > 1".into(),
                    })
                },
                failure,
            )
            .unwrap();
            assert_eq!(decision.allowed, allowed);
        }
    }
}
