// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The texts of the invitation (and task assignment) emails the bridge
//! sends (iMIP),
//! in every language it knows. All of them are here, and only here.
//!
//! **Which language** (extension of 2026-10-28): the
//! organizer's, from the `Accept-Language` of the JMAP request that made
//! the change. The web app sends its interface language there (the one
//! chosen in Settings, else the browser's), so no preference has to be
//! stored in the bridge; any other client gets its browser's or system's
//! language. Unknown or missing: English. The recipient's language is not
//! known; the `text/calendar` part, which their calendar reads, does not
//! depend on it.

use nebula_http::AcceptLanguage;

/// A language of the texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Language {
    English,
    Spanish,
}

/// The primary tags, in the order `Language::from` tries them.
const SUPPORTED: &[&str] = &["en", "es"];

impl Language {
    /// The best one for the request (English if none matches).
    pub(crate) fn for_request(languages: &AcceptLanguage) -> Self {
        match languages.best(SUPPORTED) {
            Some("es") => Self::Spanish,
            _ => Self::English,
        }
    }

    /// The `Content-Language` of the email.
    pub(crate) const fn tag(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Spanish => "es",
        }
    }

    pub(crate) const fn texts(self) -> &'static Texts {
        match self {
            Self::English => &ENGLISH,
            Self::Spanish => &SPANISH,
        }
    }
}

/// The strings of one language.
#[derive(Debug)]
pub(crate) struct Texts {
    /// Subject prefix of a new invitation.
    pub(crate) invitation: &'static str,
    /// Subject prefix of a changed one.
    pub(crate) updated: &'static str,
    /// Subject prefix of a cancellation.
    pub(crate) cancelled: &'static str,
    /// Subject prefix of a proposal of another time (`COUNTER`).
    pub(crate) proposal: &'static str,
    /// Subject prefix of a declined proposal (`DECLINECOUNTER`).
    pub(crate) proposal_declined: &'static str,
    /// Subject prefix of anything else.
    pub(crate) calendar: &'static str,
    /// Label before the date and time.
    pub(crate) when: &'static str,
    /// Label before the proposed date and time.
    pub(crate) proposed_time: &'static str,
    /// Label before the organizer.
    pub(crate) organizer: &'static str,
    /// Label before the attendee who proposes.
    pub(crate) from: &'static str,
    /// Last line of the text part of an invitation.
    pub(crate) attached: &'static str,
    /// Last line of the text part of a proposal or its answer.
    pub(crate) attached_message: &'static str,
    /// Subject prefix of a new task assignment.
    pub(crate) task_assigned: &'static str,
    /// Subject prefix of a changed one.
    pub(crate) task_updated: &'static str,
    /// Subject prefix of a task taken back.
    pub(crate) task_cancelled: &'static str,
    /// Label before a task's due date.
    pub(crate) due: &'static str,
    /// Label before who assigned the task.
    pub(crate) assigned_by: &'static str,
    /// Last line of the text part of a task.
    pub(crate) task_attached: &'static str,
}

const ENGLISH: Texts = Texts {
    invitation: "Invitation",
    updated: "Updated invitation",
    cancelled: "Cancelled",
    proposal: "New time proposed",
    proposal_declined: "Proposal declined",
    calendar: "Calendar",
    when: "When",
    proposed_time: "Proposed time",
    organizer: "Organizer",
    from: "From",
    attached: "The calendar invitation is attached (text/calendar).",
    attached_message: "The calendar message is attached (text/calendar).",
    task_assigned: "Task assigned",
    task_updated: "Task updated",
    task_cancelled: "Task cancelled",
    due: "Due",
    assigned_by: "Assigned by",
    task_attached: "The task is attached (text/calendar).",
};

const SPANISH: Texts = Texts {
    invitation: "Invitación",
    updated: "Invitación actualizada",
    cancelled: "Cancelado",
    proposal: "Nueva hora propuesta",
    proposal_declined: "Propuesta rechazada",
    calendar: "Calendario",
    when: "Cuándo",
    proposed_time: "Hora propuesta",
    organizer: "Organizador",
    from: "De",
    attached: "La invitación de calendario va adjunta (text/calendar).",
    attached_message: "El mensaje de calendario va adjunto (text/calendar).",
    task_assigned: "Tarea asignada",
    task_updated: "Tarea actualizada",
    task_cancelled: "Tarea cancelada",
    due: "Fecha límite",
    assigned_by: "Asignada por",
    task_attached: "La tarea va adjunta (text/calendar).",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice() {
        let pick = |header: &str| Language::for_request(&AcceptLanguage::parse(header));
        assert_eq!(pick("es-ES,es;q=0.9,en;q=0.8"), Language::Spanish);
        assert_eq!(pick("es"), Language::Spanish);
        assert_eq!(pick("en-GB"), Language::English);
        assert_eq!(pick("fr-FR,de;q=0.9"), Language::English);
        assert_eq!(pick("fr-FR,es;q=0.5"), Language::Spanish);
        assert_eq!(pick(""), Language::English);
        assert_eq!(Language::Spanish.tag(), "es");
    }
}
