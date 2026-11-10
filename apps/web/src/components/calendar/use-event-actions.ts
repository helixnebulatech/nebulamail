// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the calendar UI does to events: save, move, delete, answer. Each
// refreshes the views afterwards and reports the result in a toast.

import type { CalendarEventId } from "@nebulamail/jmap-client";
import { useToast } from "@nebulamail/ui";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { QueryKey } from "@tanstack/react-query";

import {
  EventSaveError,
  createEvent,
  deleteEvent,
  getEvent,
  isTooManyToNotify,
  updateEvent,
} from "../../calendar/api";
import type { EventWithSeries } from "../../calendar/api";
import type { BuildOptions, EventForm, Me } from "../../calendar/editing";
import {
  canChangeOneOccurrence,
  eventProperties,
  movedOccurrence,
  movedStart,
  shiftLocalStart,
  withoutNulls,
} from "../../calendar/editing";
import {
  calendarKeys,
  refreshEvents,
  useCalendars,
  useOwnCalendarAddresses,
  useTimeZone,
} from "../../calendar/hooks";
import type {
  Answer,
  Proposal,
  ReplyExtras,
  ReplyTexts,
} from "../../calendar/invitations";
import {
  NoSendingIdentityError,
  answerEvent,
  proposeForEvent,
} from "../../calendar/invitations";
import type { EventDetail, Occurrence } from "../../calendar/model";
import { formatDuration } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import { titleText } from "./event-text";
import type { MoveRequest } from "./time-grid";

/** How the user signs an answer: the name, or the address if it's empty. */
const displayName = (me: {
  readonly name: string | null;
  readonly email: string;
}): string => (me.name === null || me.name.trim() === "" ? me.email : me.name);

/** "Accepted: Planning" and a one-line body, in the user's language. */
export const replyTexts = (
  status: Answer,
  title: string,
  name: string
): ReplyTexts => {
  const clean = titleText(title);
  switch (status) {
    case "accepted": {
      return {
        subject: m.cal_reply_subject_accepted({ title: clean }),
        text: m.cal_reply_text_accepted({ name, title: clean }),
      };
    }
    case "tentative": {
      return {
        subject: m.cal_reply_subject_tentative({ title: clean }),
        text: m.cal_reply_text_tentative({ name, title: clean }),
      };
    }
    case "declined": {
      return {
        subject: m.cal_reply_subject_declined({ title: clean }),
        text: m.cal_reply_text_declined({ name, title: clean }),
      };
    }
    default: {
      return status satisfies never;
    }
  }
};

/** "New time proposed: Planning" and a line with the proposed time. */
export const counterTexts = (
  title: string,
  name: string,
  when: string
): ReplyTexts => {
  const clean = titleText(title);
  return {
    subject: m.cal_counter_subject({ title: clean }),
    text: m.cal_counter_text({ name, time: when, title: clean }),
  };
};

/** The organizer keeps the time (DECLINECOUNTER). */
export const declineCounterTexts = (
  title: string,
  name: string
): ReplyTexts => {
  const clean = titleText(title);
  return {
    subject: m.cal_decline_counter_subject({ title: clean }),
    text: m.cal_decline_counter_text({ name, title: clean }),
  };
};

export type Scope = "one" | "all";

/**
 * What to say when an iTIP message (answer, proposal…) wasn't sent: why,
 * if the user has no identity with the address it must come from.
 */
export const sendErrorText = (error: unknown, fallback: string): string =>
  error instanceof NoSendingIdentityError
    ? m.inv_error_no_identity({ address: error.address })
    : fallback;

const errorText = (error: unknown): string =>
  error instanceof EventSaveError && error.type === "forbidden"
    ? m.cal_error_forbidden()
    : m.cal_error_save();

export interface SaveRequest {
  readonly form: EventForm;
  readonly me: Me;
  /** Editing: the event (or occurrence) and where to apply. */
  readonly existing: {
    readonly detail: EventDetail;
    readonly scope: Scope;
    readonly participants: BuildOptions["previousParticipants"];
    readonly alerts: BuildOptions["previousAlerts"];
    readonly locations: BuildOptions["previousLocations"];
  } | null;
  readonly notify: boolean;
}

/** Occurrence lists as they were before a move (to put them back). */
type SavedLists = readonly (readonly [QueryKey, Occurrence[] | undefined])[];

export const useEventActions = () => {
  const client = useQueryClient();
  const toast = useToast();
  const zone = useTimeZone();
  const calendars = useCalendars();
  const own = useOwnCalendarAddresses();

  const done = async (): Promise<void> => {
    // Events only: the calendar list didn't change.
    await refreshEvents(client);
  };

  const save = useMutation({
    mutationFn: async (request: SaveRequest): Promise<CalendarEventId> => {
      const { existing, form, me, notify } = request;
      if (existing === null) {
        return await createEvent(
          withoutNulls(eventProperties(form, { me })),
          notify,
          form.accountId
        );
      }
      const { detail, scope } = existing;
      // Not when it turns all-day or timed (canChangeOneOccurrence).
      const occurrence =
        scope === "one" && canChangeOneOccurrence(detail, form.allDay);
      const properties = eventProperties(form, {
        me,
        occurrence,
        previousAlerts: existing.alerts ?? null,
        previousLocations: existing.locations ?? null,
        previousParticipants: existing.participants ?? null,
        previousStart: { start: detail.localStart, timeZone: detail.timeZone },
      });
      if (occurrence) {
        // The server tells the guests (Stalwart 1.0.1 and the bridge send
        // the REQUEST for one occurrence themselves).
        await updateEvent(detail.id, properties, notify, detail.accountId);
        return detail.id;
      }
      if (detail.isOccurrence && typeof properties.start === "string") {
        // The whole series from one occurrence: move the series' start by
        // as much as this occurrence moved.
        const series = await getEvent(
          detail.baseId,
          detail.baseId,
          zone,
          own.data ?? [],
          calendars.data ?? [],
          detail.accountId
        );
        const shifted =
          series?.seriesStart === null || series === null
            ? null
            : shiftLocalStart(
                series.seriesStart,
                detail.localStart,
                properties.start
              );
        if (shifted !== null) {
          properties.start = shifted;
        }
      }
      await updateEvent(detail.baseId, properties, notify, detail.accountId);
      return detail.baseId;
    },
    onError: (error) => {
      toast.show({ title: errorText(error), tone: "danger" });
    },
    onSuccess: async (_id, request) => {
      toast.show({
        title:
          request.notify && request.form.guests.length > 0
            ? m.cal_saved_sent()
            : m.cal_saved(),
      });
      await done();
    },
  });

  const remove = useMutation({
    mutationFn: async ({
      detail,
      notify,
      scope,
    }: {
      readonly detail: Pick<
        EventDetail,
        "id" | "baseId" | "isOccurrence" | "accountId"
      >;
      readonly scope: Scope;
      readonly notify: boolean;
    }) => {
      await deleteEvent(
        scope === "one" && detail.isOccurrence ? detail.id : detail.baseId,
        notify,
        detail.accountId
      );
    },
    onError: (error, request) => {
      // The caller offers to delete without notifying (event-panel).
      if (!(request.notify && isTooManyToNotify(error))) {
        toast.show({ title: errorText(error), tone: "danger" });
      }
    },
    onSuccess: async () => {
      toast.show({ title: m.cal_deleted() });
      await done();
    },
  });

  const move = useMutation({
    mutationFn: async ({
      event,
      request,
      scope,
    }: {
      /** The event as fetched when it was dropped (`eventFor`). */
      readonly event: EventWithSeries;
      readonly request: MoveRequest;
      readonly scope: Scope;
    }) => {
      const { detail } = event;
      const start = movedStart(
        detail.localStart,
        request.days,
        request.minutes
      );
      if (start === null) {
        throw new Error("Unreadable start");
      }
      const patch: Record<string, unknown> = { start };
      if (request.resize !== 0 && !detail.allDay) {
        patch.duration = formatDuration(
          0,
          Math.max(15, detail.durationMinutes + request.resize)
        );
      }
      const notify = detail.isOrganizer && detail.participants.length > 1;
      if (scope === "one" && canChangeOneOccurrence(detail, detail.allDay)) {
        if (detail.participants.length > 0) {
          // Stored in the occurrence's override with its new time:
          // without them Stalwart ignores replies to it.
          // Fetched with the occurrence when it was dropped.
          const participants = event.occurrenceParticipants;
          if (participants !== undefined && participants !== null) {
            patch.participants = participants;
          }
        }
        await updateEvent(detail.id, patch, notify, detail.accountId);
        return;
      }
      if (detail.isOccurrence) {
        // The series' start came with the occurrence (`getEvent`).
        const shifted =
          event.seriesStart === null
            ? null
            : shiftLocalStart(event.seriesStart, detail.localStart, start);
        if (shifted === null) {
          throw new Error("Unreadable series start");
        }
        patch.start = shifted;
      }
      await updateEvent(detail.baseId, patch, notify, detail.accountId);
    },
    onError: (
      error,
      _variables,
      saved: { readonly lists: SavedLists } | undefined
    ) => {
      // Back where it was.
      for (const [key, data] of saved?.lists ?? []) {
        client.setQueryData(key, data);
      }
      toast.show({ title: errorText(error), tone: "danger" });
    },
    // The event shows at once where it was dropped (one occurrence or an
    // event that doesn't repeat; a whole series waits for the server).
    onMutate: async ({
      request,
      scope,
    }): Promise<{ readonly lists: SavedLists }> => {
      const { occurrence } = request;
      if (scope === "all" && occurrence.recurring) {
        return { lists: [] };
      }
      const moved = movedOccurrence(occurrence, request, zone);
      if (moved === null) {
        return { lists: [] };
      }
      const key = calendarKeys.occurrences();
      await client.cancelQueries({ queryKey: key });
      const lists = client.getQueriesData<Occurrence[]>({ queryKey: key });
      client.setQueriesData<Occurrence[]>({ queryKey: key }, (list) =>
        list?.map((item) =>
          item.id === occurrence.id && item.accountId === occurrence.accountId
            ? moved
            : item
        )
      );
      return { lists };
    },
    onSettled: async (_data, error) => {
      if (error !== null) {
        await done();
      }
    },
    onSuccess: async () => {
      toast.show({ title: m.cal_moved() });
      await done();
    },
  });

  const answer = useMutation({
    mutationFn: async ({
      detail,
      extras,
      me,
      status,
    }: {
      readonly detail: EventDetail;
      readonly status: Answer;
      readonly me: Me;
      readonly extras?: ReplyExtras;
    }) => {
      await answerEvent(
        detail,
        status,
        replyTexts(status, detail.title, displayName(me)),
        extras
      );
    },
    onError: (error) => {
      toast.show({
        title: sendErrorText(error, m.cal_error_reply()),
        tone: "danger",
      });
    },
    onSuccess: async () => {
      toast.show({ title: m.cal_reply_sent() });
      await done();
    },
  });

  const propose = useMutation({
    mutationFn: async ({
      detail,
      extras,
      me,
      proposal,
      when,
    }: {
      readonly detail: EventDetail;
      readonly proposal: Proposal;
      readonly me: Me;
      /** The proposed time in words, for the email's text. */
      readonly when: string;
      readonly extras?: ReplyExtras;
    }) => {
      await proposeForEvent(
        detail,
        proposal,
        counterTexts(detail.title, displayName(me), when),
        extras
      );
    },
    onError: (error) => {
      toast.show({
        title: sendErrorText(error, m.cal_error_reply()),
        tone: "danger",
      });
    },
    onSuccess: () => {
      toast.show({ title: m.cal_proposal_sent() });
    },
  });

  return { answer, move, propose, remove, save };
};

/** The detail of an occurrence, fetched for a move (drag or keys). */
/**
 * An occurrence with its series (what `move` needs: the occurrence's
 * participants and the series' start), in one request.
 */
export const eventFor = async (
  occurrence: Occurrence,
  zone: string,
  own: readonly string[],
  calendars: Parameters<typeof getEvent>[4]
): Promise<EventWithSeries | null> =>
  await getEvent(
    occurrence.id,
    occurrence.baseId,
    zone,
    own,
    calendars,
    occurrence.accountId
  );
