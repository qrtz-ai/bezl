//! Summary templates (port of packages/core/src/note/templates.ts).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryTemplate {
    pub id: &'static str,
    pub label: &'static str,
    /// Extra instructions for the LLM; the JSON shape stays the same.
    pub guidance: &'static str,
    /// Words that make a sentence more relevant for the extractive fallback.
    pub focus: &'static [&'static str],
}

pub const SUMMARY_TEMPLATES: [SummaryTemplate; 5] = [
    SummaryTemplate {
        id: "general",
        label: "General",
        guidance: "Summarize the key points and decisions.",
        focus: &["decide", "decided", "agreed", "plan", "next"],
    },
    SummaryTemplate {
        id: "one-on-one",
        label: "1:1",
        guidance: "This is a 1:1. Cover how the person is doing, wins, blockers, feedback given in either direction, and career/growth topics. Action items are commitments by either person.",
        focus: &["feedback", "blocker", "blocked", "growth", "career", "feel", "goal"],
    },
    SummaryTemplate {
        id: "standup",
        label: "Standup",
        guidance: "This is a standup. In the summary, list per person: done since last time, doing next, blockers. Keep it terse. Action items are blockers needing follow-up.",
        focus: &["yesterday", "today", "blocked", "blocker", "done", "working", "finished"],
    },
    SummaryTemplate {
        id: "sales",
        label: "Sales call",
        guidance: "This is a sales call. Cover the customer's needs and pain points, budget, timeline, decision makers, objections, competitors mentioned, and agreed next steps. Q&A should capture the customer's questions.",
        focus: &["budget", "price", "pricing", "timeline", "decision", "competitor", "contract", "need", "pain"],
    },
    SummaryTemplate {
        id: "interview",
        label: "Interview",
        guidance: "This is a job interview. Summarize the candidate's background, strengths, concerns and notable answers. Q&A should pair interview questions with the candidate's answers. Do not make a hiring recommendation.",
        focus: &["experience", "project", "team", "role", "built", "led", "challenge"],
    },
];

/// `templateById`: unknown or absent ids are the general template.
pub fn template_by_id(id: Option<&str>) -> &'static SummaryTemplate {
    SUMMARY_TEMPLATES
        .iter()
        .find(|t| Some(t.id) == id)
        .unwrap_or(&SUMMARY_TEMPLATES[0])
}
